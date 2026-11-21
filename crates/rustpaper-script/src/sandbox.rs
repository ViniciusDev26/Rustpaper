// The worker's self-imposed jail, applied before it reads any script.
//
// Layers (each one alone would already stop a script from touching the
// system; together they are defense in depth if QuickJS has a bug):
// - rlimits: no new files, no child processes, no core dumps, bounded
//   address space (QuickJS also has its own heap limit).
// - PR_SET_NO_NEW_PRIVS: no privilege gain through exec (required by seccomp).
// - seccomp-bpf allowlist: memory management, futex, clocks, randomness,
//   reading stdin and writing stdout/stderr. Every other syscall (open,
//   socket, connect, execve, clone, ptrace, kill...) fails with EPERM. Memory
//   can never be made executable (no PROT_EXEC in mmap/mprotect).
//
// The host also starts the worker with an empty environment (except TZ) and
// only the three pipes as file descriptors.

#[cfg(target_os = "linux")]
pub fn lock_down(address_space: u64) -> Result<(), String> {
    use seccompiler::{
        BpfProgram, SeccompAction, SeccompCmpArgLen as Len, SeccompCmpOp as Op, SeccompCondition,
        SeccompFilter, SeccompRule,
    };
    use std::collections::BTreeMap;

    let limit = |res, v: u64| {
        let l = libc::rlimit {
            rlim_cur: v,
            rlim_max: v,
        };
        // SAFETY: plain syscall wrapper with a valid pointer.
        if unsafe { libc::setrlimit(res, &l) } != 0 {
            return Err(format!(
                "setrlimit {res}: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    };
    limit(libc::RLIMIT_NOFILE, 3)?;
    limit(libc::RLIMIT_NPROC, 0)?;
    limit(libc::RLIMIT_CORE, 0)?;
    limit(libc::RLIMIT_FSIZE, 0)?;
    limit(libc::RLIMIT_AS, address_space)?;
    // SAFETY: prctl with integer arguments.
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
        return Err(format!("no_new_privs: {}", std::io::Error::last_os_error()));
    }

    let cond =
        |arg, op, v| SeccompCondition::new(arg, Len::Qword, op, v).map_err(|e| e.to_string());
    let rule = |c: Vec<SeccompCondition>| SeccompRule::new(c).map_err(|e| e.to_string());
    let no_exec = |arg| -> Result<Vec<SeccompRule>, String> {
        Ok(vec![rule(vec![cond(
            arg,
            Op::MaskedEq(libc::PROT_EXEC as u64),
            0,
        )?])?])
    };
    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();
    // an empty rule list = allowed unconditionally
    for sys in [
        libc::SYS_munmap,
        libc::SYS_mremap,
        libc::SYS_brk,
        libc::SYS_madvise,
        libc::SYS_futex,
        libc::SYS_exit,
        libc::SYS_exit_group,
        libc::SYS_rt_sigreturn,
        libc::SYS_rt_sigprocmask,
        libc::SYS_sigaltstack,
        libc::SYS_clock_gettime,
        libc::SYS_gettimeofday,
        libc::SYS_getrandom,
        libc::SYS_sched_yield,
    ] {
        rules.insert(sys, vec![]);
    }
    rules.insert(libc::SYS_mmap, no_exec(2)?);
    rules.insert(libc::SYS_mprotect, no_exec(2)?);
    rules.insert(libc::SYS_read, vec![rule(vec![cond(0, Op::Eq, 0)?])?]);
    rules.insert(
        libc::SYS_write,
        vec![
            rule(vec![cond(0, Op::Eq, 1)?])?,
            rule(vec![cond(0, Op::Eq, 2)?])?,
        ],
    );
    let arch = std::env::consts::ARCH
        .try_into()
        .map_err(|e| format!("seccomp arch: {e:?}"))?;
    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Errno(libc::EPERM as u32),
        SeccompAction::Allow,
        arch,
    )
    .map_err(|e| e.to_string())?;
    let prog: BpfProgram = filter.try_into().map_err(|e| format!("{e:?}"))?;
    seccompiler::apply_filter(&prog).map_err(|e| e.to_string())
}

/// Run after `lock_down`: tries what a script that escaped QuickJS would
/// try. Prints one line per attempt; returns 0 when every attempt was denied.
#[cfg(target_os = "linux")]
pub fn self_test() -> i32 {
    let mut failures = 0;
    let mut check = |what: &str, ok: bool| {
        println!("{what}: {}", if ok { "denied" } else { "ALLOWED" });
        if !ok {
            failures += 1;
        }
    };
    // SAFETY: plain syscalls with valid (static) arguments; results checked.
    unsafe {
        let fd = libc::open(c"/etc/hostname".as_ptr(), libc::O_RDONLY);
        check("open", fd < 0);
        let fd = libc::openat(
            libc::AT_FDCWD,
            c"/tmp/rp-sandbox".as_ptr(),
            libc::O_CREAT | libc::O_WRONLY,
            0o600,
        );
        check("openat(create)", fd < 0);
        check(
            "socket",
            libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) < 0,
        );
        check(
            "fork",
            libc::syscall(libc::SYS_clone, libc::SIGCHLD, 0, 0, 0, 0) < 0,
        );
        let argv = [c"/bin/sh".as_ptr(), std::ptr::null()];
        check(
            "execve",
            libc::execve(c"/bin/sh".as_ptr(), argv.as_ptr(), std::ptr::null()) < 0,
        );
        check("write(fd 3)", libc::write(3, c"x".as_ptr().cast(), 1) < 0);
        check("kill(parent)", libc::kill(libc::getppid(), 0) < 0);
        let exec = libc::mmap(
            std::ptr::null_mut(),
            4096,
            libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        );
        check("mmap(PROT_EXEC)", exec == libc::MAP_FAILED);
        let page = libc::mmap(
            std::ptr::null_mut(),
            4096,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        );
        check(
            "mprotect(PROT_EXEC)",
            page == libc::MAP_FAILED
                || libc::mprotect(page, 4096, libc::PROT_READ | libc::PROT_EXEC) < 0,
        );
    }
    failures
}

#[cfg(not(target_os = "linux"))]
pub fn self_test() -> i32 {
    1
}

/// Other systems have no seccomp: refuse to run scripts at all.
#[cfg(not(target_os = "linux"))]
pub fn lock_down(_address_space: u64) -> Result<(), String> {
    Err("SceneScript sandbox is only implemented on Linux".into())
}
