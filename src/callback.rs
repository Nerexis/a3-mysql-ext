use std::{
    ffi::{CString, c_char, c_int},
    sync::{Mutex, OnceLock},
};

#[cfg(windows)]
pub type CallbackProc =
    unsafe extern "system" fn(*const c_char, *const c_char, *const c_char) -> c_int;

#[cfg(not(windows))]
pub type CallbackProc = unsafe extern "C" fn(*const c_char, *const c_char, *const c_char) -> c_int;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NotifyError {
    Unavailable,
    QueueFull,
}

static CALLBACK: OnceLock<Mutex<Option<CallbackProc>>> = OnceLock::new();

fn callback_slot() -> &'static Mutex<Option<CallbackProc>> {
    CALLBACK.get_or_init(|| Mutex::new(None))
}

pub fn register(callback: Option<CallbackProc>) {
    *callback_slot()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = callback;
}

pub fn is_registered() -> bool {
    callback_slot()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .is_some()
}

pub fn notify_result_ready(id: u64) -> Result<c_int, NotifyError> {
    let callback = *callback_slot()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(callback) = callback else {
        return Err(NotifyError::Unavailable);
    };
    invoke(callback, id)
}

fn invoke(callback: CallbackProc, id: u64) -> Result<c_int, NotifyError> {
    let data = CString::new(format!("[{id}]")).expect("request identifiers contain no NUL bytes");
    let slots_left = unsafe {
        callback(
            c"extDB3".as_ptr(),
            c"asyncResultReady".as_ptr(),
            data.as_ptr(),
        )
    };
    if slots_left < 0 {
        Err(NotifyError::QueueFull)
    } else {
        Ok(slots_left)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;

    static LAST_EVENT: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

    fn capture(name: *const c_char, function: *const c_char, data: *const c_char) -> c_int {
        let values = [name, function, data]
            .into_iter()
            .map(|value| {
                unsafe { CStr::from_ptr(value) }
                    .to_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        *LAST_EVENT
            .get_or_init(|| Mutex::new(Vec::new()))
            .lock()
            .unwrap() = values;
        7
    }

    #[cfg(windows)]
    unsafe extern "system" fn capture_callback(
        name: *const c_char,
        function: *const c_char,
        data: *const c_char,
    ) -> c_int {
        capture(name, function, data)
    }

    #[cfg(not(windows))]
    unsafe extern "C" fn capture_callback(
        name: *const c_char,
        function: *const c_char,
        data: *const c_char,
    ) -> c_int {
        capture(name, function, data)
    }

    #[test]
    fn callback_event_uses_stable_name_function_and_array_data() {
        assert_eq!(invoke(capture_callback, 42), Ok(7));
        let values = LAST_EVENT.get().unwrap().lock().unwrap().clone();
        assert_eq!(values, ["extDB3", "asyncResultReady", "[42]"]);
    }
}
