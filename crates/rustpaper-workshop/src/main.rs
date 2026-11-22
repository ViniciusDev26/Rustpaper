// rustpaper-workshop: the selector's bridge to the Steam Workshop.
//
// It talks to the running Steam client through the Steamworks API as
// Wallpaper Engine (AppID 431960), the same calls WE itself makes: browse
// and search the Workshop, subscribe / unsubscribe (Steam itself downloads
// into steamapps/workshop/content/431960, where the gallery already looks)
// and report download progress. No API key or login: it uses the Steam
// session, so Steam must be running and the account must own WE.
//
// A separate process, spawned by the selector on demand: while the
// Steamworks API is up, Steam shows the user as "playing Wallpaper Engine",
// so it exits after IDLE without requests (and with the selector, see
// spawn in rustpaper-selector). It runs no Workshop content.
//
// Protocol (JSON lines on stdin/stdout): see protocol.rs.

mod protocol;

use std::io::{BufRead, Write};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use protocol::{Command, Item, Query, Request, State};
use serde_json::{Value, json};
use steamworks::{
    AppIDs, AppId, Client, ItemState, PublishedFileId, UGCQueryType, UGCStatisticType, UGCType,
    UserList, UserListOrder,
};

const WALLPAPER_ENGINE: AppId = AppId(431960);
/// Exit after this long without a request (the selector respawns on demand).
const IDLE: Duration = Duration::from_secs(300);
/// How often Steam callbacks are pumped while waiting for requests.
const PUMP: Duration = Duration::from_millis(50);

fn main() {
    let (out_tx, out_rx) = mpsc::channel::<String>();
    // stdout writer: callbacks answer from the main thread too, all through
    // this channel, so lines never interleave
    let writer = std::thread::spawn(move || {
        let mut stdout = std::io::stdout().lock();
        for line in out_rx {
            if writeln!(stdout, "{line}")
                .and_then(|_| stdout.flush())
                .is_err()
            {
                break; // selector gone
            }
        }
    });

    let client = match Client::init_app(WALLPAPER_ENGINE) {
        Ok(c) => c,
        Err(e) => {
            let _ = out_tx.send(json!({ "ready": false, "error": init_error(&e) }).to_string());
            drop(out_tx);
            let _ = writer.join();
            std::process::exit(1);
        }
    };
    let _ = out_tx.send(json!({ "ready": true }).to_string());

    let (req_tx, req_rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if req_tx.send(line).is_err() {
                break;
            }
        }
        // EOF: the selector closed the pipe; the main loop sees the channel
        // disconnect and exits
    });

    serve(&client, &req_rx, &out_tx);
    drop(out_tx);
    let _ = writer.join();
}

fn init_error(e: &steamworks::SteamAPIInitError) -> String {
    format!(
        "could not connect to Steam ({e}). Start Steam, signed in to an account that owns Wallpaper Engine."
    )
}

/// A subscriptions page to fetch next: (request id, page, entries so far).
/// Steam callbacks must not start queries themselves (run_callbacks holds
/// the callback registry's lock: registering a new call there deadlocks),
/// so a page callback queues the next page and the main loop sends it.
type NextPage = (u64, u32, serde_json::Map<String, Value>);

fn serve(client: &Client, requests: &Receiver<String>, out: &Sender<String>) {
    let mut last_request = Instant::now();
    let (next_tx, next_rx) = mpsc::channel::<NextPage>();
    loop {
        client.run_callbacks();
        for (id, page, acc) in next_rx.try_iter() {
            subscriptions(client, id, page, acc, out.clone(), next_tx.clone());
        }
        match requests.recv_timeout(PUMP) {
            Ok(line) => {
                last_request = Instant::now();
                match serde_json::from_str::<Request>(&line) {
                    Ok(r) => handle(client, r, out, &next_tx),
                    Err(e) => {
                        let id = serde_json::from_str::<Value>(&line)
                            .ok()
                            .and_then(|v| v["id"].as_u64())
                            .unwrap_or(0);
                        let _ = out.send(protocol::err(id, format!("bad request: {e}")));
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if last_request.elapsed() > IDLE {
                    return;
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn handle(client: &Client, r: Request, out: &Sender<String>, next: &Sender<NextPage>) {
    let id = r.id;
    let reply = |res: Result<Value, String>| match res {
        Ok(v) => protocol::ok(id, v),
        Err(e) => protocol::err(id, e),
    };
    match r.cmd {
        Command::Query(q) => query(client, id, &q, out.clone()),
        Command::Subscribe { item } | Command::Unsubscribe { item }
            if protocol::parse_item(&item).is_err() =>
        {
            let _ = out.send(reply(protocol::parse_item(&item).map(|_| Value::Null)));
        }
        Command::Subscribe { item } => {
            let file = PublishedFileId(protocol::parse_item(&item).unwrap_or_default());
            let ugc = client.ugc();
            let out = out.clone();
            ugc.subscribe_item(file, move |res| {
                let _ = out.send(reply_for(id, res.map(|_| Value::Null)));
            });
            // start right away instead of waiting for Steam's queue
            ugc.download_item(file, true);
        }
        Command::Unsubscribe { item } => {
            let file = PublishedFileId(protocol::parse_item(&item).unwrap_or_default());
            let out = out.clone();
            client.ugc().unsubscribe_item(file, move |res| {
                let _ = out.send(reply_for(id, res.map(|_| Value::Null)));
            });
        }
        Command::Subscriptions => subscriptions(
            client,
            id,
            1,
            serde_json::Map::new(),
            out.clone(),
            next.clone(),
        ),
        Command::Status { items } => {
            let states: serde_json::Map<String, Value> = items
                .iter()
                .filter_map(|i| protocol::parse_item(i).ok().map(|v| (i.clone(), v)))
                .map(|(key, v)| {
                    let state = state_of(client, PublishedFileId(v));
                    (key, serde_json::to_value(state).unwrap_or_default())
                })
                .collect();
            let _ = out.send(reply(Ok(Value::Object(states))));
        }
    }
}

/// Steam's page size for UGC queries.
const PAGE: u32 = 50;
/// Safety cap on subscription pages (5000 items).
const MAX_PAGES: u32 = 100;

/// Every subscription with the time it was added to the list, one page at
/// a time (each page's callback queues the next one, see NextPage).
fn subscriptions(
    client: &Client,
    id: u64,
    page: u32,
    mut acc: serde_json::Map<String, Value>,
    out: Sender<String>,
    next: Sender<NextPage>,
) {
    let account = client.user().steam_id().account_id();
    let apps = AppIDs::Both {
        creator: WALLPAPER_ENGINE,
        consumer: WALLPAPER_ENGINE,
    };
    let handle = client.ugc().query_user(
        account,
        UserList::Subscribed,
        UGCType::Items,
        UserListOrder::SubscriptionDateDesc,
        apps,
        page,
    );
    let Ok(handle) = handle else {
        let _ = out.send(protocol::err(id, "Steam refused the subscriptions query"));
        return;
    };
    handle.fetch(move |res| match res {
        Err(e) => {
            let _ = out.send(protocol::err(id, e));
        }
        Ok(results) => {
            for r in results.iter().flatten() {
                acc.insert(
                    r.published_file_id.0.to_string(),
                    json!(r.time_added_to_user_list),
                );
            }
            let done = results.returned_results() < PAGE
                || page * PAGE >= results.total_results()
                || page >= MAX_PAGES;
            if done {
                let _ = out.send(protocol::ok(id, Value::Object(acc)));
            } else {
                let _ = next.send((id, page + 1, acc));
            }
        }
    });
}

fn reply_for(id: u64, res: Result<Value, steamworks::SteamError>) -> String {
    match res {
        Ok(v) => protocol::ok(id, v),
        Err(e) => protocol::err(id, e),
    }
}

fn state_of(client: &Client, file: PublishedFileId) -> State {
    let ugc = client.ugc();
    let s = ugc.item_state(file);
    let (downloaded, total) = ugc.item_download_info(file).unwrap_or((0, 0));
    State {
        subscribed: s.contains(ItemState::SUBSCRIBED),
        installed: s.contains(ItemState::INSTALLED),
        downloading: s.intersects(ItemState::DOWNLOADING | ItemState::DOWNLOAD_PENDING),
        needs_update: s.contains(ItemState::NEEDS_UPDATE),
        downloaded,
        total,
    }
}

fn query(client: &Client, id: u64, q: &Query, out: Sender<String>) {
    let ugc = client.ugc();
    let apps = AppIDs::Both {
        creator: WALLPAPER_ENGINE,
        consumer: WALLPAPER_ENGINE,
    };
    let page = q.page.max(1);
    let handle = if q.sort == "subscribed" {
        let account = client.user().steam_id().account_id();
        ugc.query_user(
            account,
            UserList::Subscribed,
            UGCType::Items,
            UserListOrder::SubscriptionDateDesc,
            apps,
            page,
        )
    } else {
        let order = match (q.search.trim().is_empty(), q.sort.as_str()) {
            (false, _) => UGCQueryType::RankedByTextSearch,
            (true, "top") => UGCQueryType::RankedByVote,
            (true, "recent") => UGCQueryType::RankedByPublicationDate,
            (true, "updated") => UGCQueryType::RankedByLastUpdatedDate,
            (true, "subscriptions") => UGCQueryType::RankedByTotalUniqueSubscriptions,
            _ => UGCQueryType::RankedByTrend,
        };
        ugc.query_all(order, UGCType::Items, apps, page)
    };
    let Ok(mut handle) = handle else {
        let _ = out.send(protocol::err(id, "Steam refused the Workshop query"));
        return;
    };
    let subscribed = q.sort == "subscribed";
    // Tag filters exist only for "all items" queries (Steam rejects
    // SetMatchAnyTag on a user list): the subscriptions list is filtered
    // below, on the results.
    if !subscribed {
        // "any of" the kind tags, minus Applications and (unless asked) the
        // mature ratings
        handle = handle.any_required(true);
        for tag in protocol::required_kind_tags(&q.kind) {
            handle = handle.require_tag(tag);
        }
        handle = handle.exclude_tag("Application");
        if !q.mature {
            for tag in protocol::MATURE_TAGS {
                handle = handle.exclude_tag(tag);
            }
        }
        if !q.search.trim().is_empty() {
            handle = handle.set_search_text(q.search.trim());
        }
        // only valid for RankedByTrend (a text search ranks by relevance)
        if q.sort == "trend" && q.search.trim().is_empty() {
            handle = handle.set_ranked_by_trend_days(7);
        }
    }
    let wanted = protocol::required_kind_tags(&q.kind);
    let mature = q.mature;
    let client = client.clone();
    handle.fetch(move |res| {
        let line = match res {
            Err(e) => protocol::err(id, e),
            Ok(results) => {
                let items: Vec<Item> = (0..results.returned_results())
                    .filter_map(|i| {
                        let r = results.get(i)?;
                        Some(Item {
                            id: r.published_file_id.0.to_string(),
                            kind: protocol::kind_of(&r.tags),
                            rating: protocol::rating_of(&r.tags),
                            title: r.title,
                            preview_url: results.preview_url(i).unwrap_or_default(),
                            tags: r.tags.into_iter().filter(|t| !t.is_empty()).collect(),
                            subscriptions: results
                                .statistic(i, UGCStatisticType::Subscriptions)
                                .unwrap_or(0),
                            votes_up: r.num_upvotes,
                            votes_down: r.num_downvotes,
                            file_size: r.file_size,
                            time_updated: r.time_updated,
                            state: state_of(&client, r.published_file_id),
                        })
                    })
                    // only wallpapers the gallery has a type for, and the
                    // filters the subscriptions query could not apply
                    .filter(|it| {
                        !it.kind.is_empty()
                            && wanted.iter().any(|k| k.eq_ignore_ascii_case(&it.kind))
                            && (mature || it.rating == "everyone")
                    })
                    .collect();
                protocol::ok(
                    id,
                    json!({ "total": results.total_results(), "page": page, "items": items }),
                )
            }
        };
        let _ = out.send(line);
    });
}
