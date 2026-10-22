//! A small, allocation-conscious libpam binding.
//!
//! We declare only the PAM ABI surface we use instead of pulling in a bindgen
//! dependency at build time. All unsafe code lives in this module, the
//! conversation buffer is allocated with libc's allocator (PAM owns and frees
//! it), and the password copy is zeroized on every path.
//!
//! This module is used by the lockscreen, where PAM authenticates the current
//! user in-process. Login session startup is delegated to greetd; a greeter
//! must not run as root just to call `setuid`/`exec`.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::fmt;
use std::ptr;

const PAM_SUCCESS: c_int = 0;
const PAM_PERM_DENIED: c_int = 6;
const PAM_AUTH_ERR: c_int = 7;
const PAM_CRED_INSUFFICIENT: c_int = 8;
const PAM_AUTHINFO_UNAVAIL: c_int = 9;
const PAM_USER_UNKNOWN: c_int = 10;
const PAM_MAXTRIES: c_int = 11;
const PAM_CONV_ERR: c_int = 19;
const PAM_AUTHTOK_ERR: c_int = 20;

const PAM_PROMPT_ECHO_OFF: c_int = 1;
const PAM_PROMPT_ECHO_ON: c_int = 2;
const PAM_ERROR_MSG: c_int = 3;
const PAM_TEXT_INFO: c_int = 4;

const PAM_SILENT: c_int = 0x8000;
const PAM_DISALLOW_NULL_AUTHTOK: c_int = 0x0001;

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

type ConversationFn = unsafe extern "C" fn(
    num_msg: c_int,
    msg: *mut *const PamMessage,
    resp: *mut *mut PamResponse,
    appdata_ptr: *mut c_void,
) -> c_int;

#[repr(C)]
struct PamConv {
    conv: Option<ConversationFn>,
    appdata_ptr: *mut c_void,
}

#[repr(C)]
struct PamHandle {
    _private: [u8; 0],
}

#[link(name = "pam")]
unsafe extern "C" {
    fn pam_start(
        service_name: *const c_char,
        user: *const c_char,
        pam_conversation: *const PamConv,
        pamh: *mut *mut PamHandle,
    ) -> c_int;
    fn pam_end(pamh: *mut PamHandle, pam_status: c_int) -> c_int;
    fn pam_authenticate(pamh: *mut PamHandle, flags: c_int) -> c_int;
    fn pam_strerror(pamh: *mut PamHandle, errnum: c_int) -> *const c_char;
}

unsafe extern "C" {
    fn calloc(nmemb: usize, size: usize) -> *mut c_void;
    fn malloc(size: usize) -> *mut c_void;
    fn free(ptr: *mut c_void);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PamError {
    InvalidCredentials,
    AccountExpired,
    PasswordExpired,
    ServiceUnavailable,
    Configuration,
    Conversation,
    Internal(c_int),
}

impl PamError {
    /// Stable, non-enumerating text for the UI. Never reveals whether the
    /// username exists.
    #[must_use]
    pub const fn user_message(self) -> &'static str {
        match self {
            Self::InvalidCredentials => "Credenziali non valide",
            Self::AccountExpired => "Account non più valido",
            Self::PasswordExpired => "Password scaduta",
            Self::ServiceUnavailable => "Servizio di autenticazione non disponibile",
            Self::Configuration | Self::Conversation | Self::Internal(_) => {
                "Autenticazione non riuscita"
            }
        }
    }
}

impl fmt::Display for PamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.user_message())
    }
}

impl std::error::Error for PamError {}

/// Authenticate against `service` (a file in `/etc/pam.d`).
///
/// The caller must enforce a cooldown/rate limit; PAM's own `pam_faillock`
/// should also be configured in the service file.
pub fn authenticate(
    service: &str,
    username: &str,
    password: &str,
    allow_empty_password: bool,
) -> Result<(), PamError> {
    if !valid_service(service) {
        return Err(PamError::Configuration);
    }
    if !valid_username(username) {
        return Err(PamError::InvalidCredentials);
    }
    if password.is_empty() && !allow_empty_password {
        return Err(PamError::InvalidCredentials);
    }
    if password.len() > 4096 || password.as_bytes().contains(&0) {
        return Err(PamError::InvalidCredentials);
    }

    let service_c = CString::new(service).map_err(|_| PamError::Configuration)?;
    let user_c = CString::new(username).map_err(|_| PamError::InvalidCredentials)?;

    // The conversation callback needs a NUL-terminated copy. We keep ownership
    // of the Vec and zeroize it as soon as PAM returns.
    let mut password_bytes = Vec::with_capacity(password.len() + 1);
    password_bytes.extend_from_slice(password.as_bytes());
    password_bytes.push(0);
    // SAFETY: exactly one trailing NUL, no interior NUL (checked above).
    let password_c = unsafe { CString::from_vec_unchecked(password_bytes) };

    let result = run_pam(&service_c, &user_c, &password_c);

    let mut bytes = password_c.into_bytes();
    zeroize(&mut bytes);
    result
}

fn run_pam(service: &CStr, user: &CStr, password: &CStr) -> Result<(), PamError> {
    let mut credentials = Credentials { user, password };
    let conv = PamConv {
        conv: Some(conversation),
        appdata_ptr: &mut credentials as *mut Credentials<'_> as *mut c_void,
    };

    let mut raw: *mut PamHandle = ptr::null_mut();
    // SAFETY: valid NUL-terminated strings and a live conversation struct.
    let status = unsafe { pam_start(service.as_ptr(), user.as_ptr(), &conv, &mut raw) };
    if status != PAM_SUCCESS || raw.is_null() {
        return Err(PamError::Configuration);
    }
    let handle = PamHandleGuard(raw);

    // SAFETY: handle is a valid PAM handle created above.
    let status = unsafe { pam_authenticate(handle.0, PAM_SILENT | PAM_DISALLOW_NULL_AUTHTOK) };
    if status != PAM_SUCCESS {
        return Err(map_status(handle.0, status));
    }

    // Deliberately no `pam_acct_mgmt` here.
    //
    // A lockscreen runs as the already-authenticated user; on a default
    // `/etc/shadow` it may not have permission to read password-expiry data,
    // which makes pam_unix's account component fail and would permanently
    // brick unlocking. Account policy belongs to greetd/PAM at login time.
    // See docs/pam.d/lard-greeter.

    Ok(())
}

struct Credentials<'a> {
    user: &'a CStr,
    password: &'a CStr,
}

struct PamHandleGuard(*mut PamHandle);

impl Drop for PamHandleGuard {
    fn drop(&mut self) {
        // SAFETY: the pointer came from a successful pam_start and is dropped
        // exactly once here.
        unsafe {
            pam_end(self.0, PAM_SUCCESS);
        }
    }
}

unsafe extern "C" fn conversation(
    num_msg: c_int,
    messages: *mut *const PamMessage,
    responses: *mut *mut PamResponse,
    appdata: *mut c_void,
) -> c_int {
    if num_msg <= 0 || messages.is_null() || responses.is_null() || appdata.is_null() {
        return PAM_CONV_ERR;
    }

    // SAFETY: appdata points to `Credentials` for the duration of the PAM call.
    let credentials = unsafe { &*(appdata as *const Credentials<'_>) };
    let count = num_msg as usize;
    // SAFETY: PAM frees this array (and each response) with free().
    let array = unsafe { calloc(count, std::mem::size_of::<PamResponse>()) } as *mut PamResponse;
    if array.is_null() {
        return PAM_CONV_ERR;
    }

    for index in 0..count {
        // SAFETY: PAM guarantees `num_msg` valid message pointers.
        let message = unsafe { *messages.add(index) };
        if message.is_null() {
            return fail(array, count, responses);
        }
        // SAFETY: message points to a live pam_message for this call.
        let style = unsafe { (*message).msg_style };
        let source = match style {
            PAM_PROMPT_ECHO_OFF => credentials.password.as_ptr(),
            PAM_PROMPT_ECHO_ON => credentials.user.as_ptr(),
            PAM_ERROR_MSG | PAM_TEXT_INFO => {
                // A NULL response is valid for informational messages.
                continue;
            }
            _ => return fail(array, count, responses),
        };
        // SAFETY: source is a NUL-terminated string owned by PAM for this call.
        let copy = unsafe { copy_c_string(source) };
        if copy.is_null() {
            return fail(array, count, responses);
        }
        // SAFETY: index is within the calloc'ed array.
        unsafe {
            (*array.add(index)).resp = copy;
            (*array.add(index)).resp_retcode = 0;
        }
    }

    // SAFETY: responses is non-null (checked above).
    unsafe {
        *responses = array;
    }
    PAM_SUCCESS
}

/// # Safety
/// `source` must point to a NUL-terminated string. The returned pointer is
/// allocated with `malloc` and ownership is transferred to PAM.
unsafe fn copy_c_string(source: *const c_char) -> *mut c_char {
    // SAFETY: caller guarantees NUL termination.
    let bytes = unsafe { CStr::from_ptr(source) }.to_bytes_with_nul();
    // SAFETY: libc allocation; PAM frees it.
    let buffer = unsafe { malloc(bytes.len()) } as *mut u8;
    if buffer.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: both buffers are valid for bytes.len() bytes.
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, bytes.len());
    }
    buffer as *mut c_char
}

fn fail(array: *mut PamResponse, count: usize, responses: *mut *mut PamResponse) -> c_int {
    // SAFETY: array has `count` responses; only `resp` fields we allocated are
    // freed. PAM never sees this array.
    unsafe {
        for index in 0..count {
            let response = (*array.add(index)).resp;
            if !response.is_null() {
                free(response as *mut c_void);
            }
        }
        free(array as *mut c_void);
        if !responses.is_null() {
            *responses = ptr::null_mut();
        }
    }
    PAM_CONV_ERR
}

fn map_status(handle: *mut PamHandle, status: c_int) -> PamError {
    match status {
        PAM_AUTH_ERR
        | PAM_PERM_DENIED
        | PAM_USER_UNKNOWN
        | PAM_MAXTRIES
        | PAM_CRED_INSUFFICIENT => PamError::InvalidCredentials,
        PAM_AUTHINFO_UNAVAIL => PamError::ServiceUnavailable,
        PAM_AUTHTOK_ERR => PamError::InvalidCredentials,
        PAM_CONV_ERR => PamError::Conversation,
        other => {
            // Read-only diagnostic string; never shown verbatim to the user.
            // SAFETY: handle is valid; pam_strerror tolerates unknown codes.
            let _ = unsafe { pam_strerror(handle, other) };
            PamError::Internal(other)
        }
    }
}

#[must_use]
pub fn valid_service(service: &str) -> bool {
    !service.is_empty()
        && service.len() <= 64
        && service
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[must_use]
pub fn valid_username(username: &str) -> bool {
    !username.is_empty()
        && username.len() <= 256
        && !username.chars().any(|c| c == '\0' || c.is_control())
}

fn zeroize(bytes: &mut [u8]) {
    for byte in bytes {
        // Volatile write: the compiler must not elide clearing the password.
        unsafe { ptr::write_volatile(byte, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_validation_rejects_paths() {
        assert!(valid_service("lard-greeter"));
        assert!(!valid_service("../../etc/pam.d/su"));
        assert!(!valid_service(""));
        assert!(!valid_service("a b"));
    }

    #[test]
    fn username_validation() {
        assert!(valid_username("alice"));
        assert!(valid_username("mario.rossi"));
        assert!(!valid_username(""));
        assert!(!valid_username("a\nb"));
        assert!(!valid_username("a\0b"));
    }

    #[test]
    fn error_messages_are_generic() {
        assert_eq!(
            PamError::Internal(99).user_message(),
            "Autenticazione non riuscita"
        );
        assert!(!PamError::Internal(99).user_message().contains("99"));
    }
}
