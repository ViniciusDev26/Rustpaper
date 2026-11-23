//! The lock screen's password check: minimal libpam FFI (like gbm.rs, a few
//! functions instead of a dependency).
//!
//! Every PAM prompt that hides its echo is answered with the typed password
//! (pam_unix asks once; stacks like pam_systemd_home ask the same way).
//! Informational and error messages are returned for the panel. The PAM
//! service comes from the lock adapter (lock/adapter.rs): Omarchy's
//! `omarchy-lock-password` keeps its faillock policy, elsewhere the packaged
//! `rustpaper-lock`.

use std::ffi::{CStr, CString, c_char, c_int, c_void};

#[repr(C)]
struct PamMessage {
    msg_style: c_int,
    msg: *const c_char,
}

#[repr(C)]
struct PamResponse {
    resp: *mut c_char,
    resp_retcode: c_int,
}

type ConvFn = unsafe extern "C" fn(
    c_int,
    *mut *const PamMessage,
    *mut *mut PamResponse,
    *mut c_void,
) -> c_int;

#[repr(C)]
struct PamConv {
    conv: ConvFn,
    appdata_ptr: *mut c_void,
}

#[repr(C)]
struct PamHandle {
    _private: [u8; 0],
}

#[link(name = "pam")]
unsafe extern "C" {
    fn pam_start(
        service: *const c_char,
        user: *const c_char,
        conv: *const PamConv,
        pamh: *mut *mut PamHandle,
    ) -> c_int;
    fn pam_start_confdir(
        service: *const c_char,
        user: *const c_char,
        conv: *const PamConv,
        confdir: *const c_char,
        pamh: *mut *mut PamHandle,
    ) -> c_int;
    fn pam_authenticate(pamh: *mut PamHandle, flags: c_int) -> c_int;
    fn pam_acct_mgmt(pamh: *mut PamHandle, flags: c_int) -> c_int;
    fn pam_setcred(pamh: *mut PamHandle, flags: c_int) -> c_int;
    fn pam_end(pamh: *mut PamHandle, status: c_int) -> c_int;
    fn pam_strerror(pamh: *mut PamHandle, errnum: c_int) -> *const c_char;
}

const PAM_SUCCESS: c_int = 0;
const PAM_BUF_ERR: c_int = 5;
const PAM_CONV_ERR: c_int = 19;
const PAM_NEW_AUTHTOK_REQD: c_int = 12;
const PAM_PROMPT_ECHO_OFF: c_int = 1;
const PAM_PROMPT_ECHO_ON: c_int = 2;
const PAM_ERROR_MSG: c_int = 3;
const PAM_TEXT_INFO: c_int = 4;
const PAM_REFRESH_CRED: c_int = 0x0010;

struct Conversation<'a> {
    password: &'a CStr,
    messages: Vec<String>,
}

/// Answers PAM's prompts (called by libpam, on the authenticating thread).
unsafe extern "C" fn converse(
    n: c_int,
    msgs: *mut *const PamMessage,
    resp: *mut *mut PamResponse,
    data: *mut c_void,
) -> c_int {
    if n <= 0 || msgs.is_null() || resp.is_null() || data.is_null() {
        return PAM_CONV_ERR;
    }
    // SAFETY: `data` is the Conversation that `authenticate` passed in.
    let conv = unsafe { &mut *(data as *mut Conversation) };
    // libpam frees the array and each answer with free(3)
    // SAFETY: calloc of n zeroed responses.
    let out =
        unsafe { libc::calloc(n as usize, std::mem::size_of::<PamResponse>()) } as *mut PamResponse;
    if out.is_null() {
        return PAM_BUF_ERR;
    }
    for i in 0..n as usize {
        // Linux-PAM passes an array of pointers to messages
        // SAFETY: libpam hands `n` valid message pointers.
        let m = unsafe { &**msgs.add(i) };
        let text = if m.msg.is_null() {
            String::new()
        } else {
            // SAFETY: a NUL-terminated string from libpam.
            unsafe { CStr::from_ptr(m.msg) }
                .to_string_lossy()
                .into_owned()
        };
        match m.msg_style {
            PAM_PROMPT_ECHO_OFF | PAM_PROMPT_ECHO_ON => {
                // SAFETY: strdup of our NUL-terminated password; freed by libpam.
                let answer = unsafe { libc::strdup(conv.password.as_ptr()) };
                if answer.is_null() {
                    // SAFETY: free what was allocated so far, then the array.
                    unsafe { free_responses(out, i) };
                    return PAM_BUF_ERR;
                }
                // SAFETY: index i < n of our array.
                unsafe { (*out.add(i)).resp = answer };
            }
            PAM_ERROR_MSG | PAM_TEXT_INFO => {
                if !text.is_empty() {
                    conv.messages.push(text);
                }
            }
            _ => {
                // SAFETY: as above.
                unsafe { free_responses(out, i) };
                return PAM_CONV_ERR;
            }
        }
    }
    // SAFETY: libpam takes ownership of the array.
    unsafe { *resp = out };
    PAM_SUCCESS
}

/// Frees the first `filled` answers (zeroing the password copies) and the
/// array.
unsafe fn free_responses(out: *mut PamResponse, filled: usize) {
    for i in 0..filled {
        // SAFETY: our own calloc'ed array; answers from strdup.
        unsafe {
            let r = (*out.add(i)).resp;
            if !r.is_null() {
                let len = libc::strlen(r);
                std::ptr::write_bytes(r, 0, len);
                libc::free(r.cast());
            }
        }
    }
    // SAFETY: as above.
    unsafe { libc::free(out.cast()) };
}

/// The login name of the user running us.
pub fn current_user() -> Option<String> {
    // SAFETY: getpwuid returns static storage or NULL; we copy right away.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() || (*pw).pw_name.is_null() {
            return None;
        }
        Some(CStr::from_ptr((*pw).pw_name).to_string_lossy().into_owned())
    }
}

/// Checks `password` for the current user with PAM service `service`.
/// Blocking (PAM sleeps after a failure): run it off the UI thread. Err
/// carries what to show (PAM's own message when it gave one).
pub fn authenticate(service: &str, password: &[u8]) -> Result<(), String> {
    let user = current_user().ok_or("unknown user")?;
    let user = CString::new(user).map_err(|_| "bad user name")?;
    let service = CString::new(service).map_err(|_| "bad PAM service")?;
    let password = CString::new(password).map_err(|_| "the password contains a NUL")?;
    let mut conv_data = Conversation {
        password: &password,
        messages: Vec::new(),
    };
    let conv = PamConv {
        conv: converse,
        appdata_ptr: (&mut conv_data as *mut Conversation).cast(),
    };
    let mut pamh: *mut PamHandle = std::ptr::null_mut();
    // Debug builds only: a PAM config directory of our own, so tests can
    // check both outcomes with pam_permit / pam_deny without root.
    let confdir = cfg!(debug_assertions)
        .then(|| std::env::var("RUSTPAPER_LOCK_PAM_CONFDIR").ok())
        .flatten()
        .and_then(|d| CString::new(d).ok());
    // SAFETY: valid C strings and conv; pamh receives the handle.
    let rc = unsafe {
        match &confdir {
            Some(dir) => pam_start_confdir(
                service.as_ptr(),
                user.as_ptr(),
                &conv,
                dir.as_ptr(),
                &mut pamh,
            ),
            None => pam_start(service.as_ptr(), user.as_ptr(), &conv, &mut pamh),
        }
    };
    if rc != PAM_SUCCESS || pamh.is_null() {
        return Err(format!("PAM could not start (service {service:?})"));
    }
    // SAFETY: a live handle from pam_start, ended exactly once below.
    let rc = unsafe {
        let mut rc = pam_authenticate(pamh, 0);
        if rc == PAM_SUCCESS {
            rc = pam_acct_mgmt(pamh, 0);
            // an expired password still proves who is at the keyboard
            if rc == PAM_NEW_AUTHTOK_REQD {
                rc = PAM_SUCCESS;
            }
        }
        if rc == PAM_SUCCESS {
            // renew Kerberos tickets and the like; failing is not fatal
            pam_setcred(pamh, PAM_REFRESH_CRED);
        }
        rc
    };
    let error = (rc != PAM_SUCCESS).then(|| {
        conv_data.messages.last().cloned().unwrap_or_else(|| {
            // SAFETY: pam_strerror returns a static string.
            unsafe { CStr::from_ptr(pam_strerror(pamh, rc)) }
                .to_string_lossy()
                .into_owned()
        })
    });
    // SAFETY: ends the handle from pam_start.
    unsafe { pam_end(pamh, rc) };
    // the CString's bytes: overwrite before they are freed
    let mut bytes = password.into_bytes_with_nul();
    bytes.fill(0);
    match error {
        None => Ok(()),
        Some(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both outcomes through a config directory of our own (debug builds).
    #[test]
    fn permit_and_deny() {
        let dir = std::env::temp_dir().join(format!("rp-pam-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("rp-permit"),
            "auth required pam_permit.so\naccount required pam_permit.so\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("rp-deny"),
            "auth required pam_deny.so\naccount required pam_permit.so\n",
        )
        .unwrap();
        // SAFETY: only this test reads the variable.
        unsafe { std::env::set_var("RUSTPAPER_LOCK_PAM_CONFDIR", &dir) };
        assert_eq!(authenticate("rp-permit", b"anything"), Ok(()));
        assert!(authenticate("rp-deny", b"anything").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
