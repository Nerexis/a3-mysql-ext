use crate::compat::error_code;
use crate::engine;
use std::{
    ffi::{CStr, c_char, c_int},
    panic::{AssertUnwindSafe, catch_unwind},
};

const MAX_ARGS: u32 = 2048;

unsafe fn read<'a>(input: *const c_char) -> Result<&'a str, String> {
    if input.is_null() {
        return Err(engine::error(error_code::INVALID_MESSAGE));
    }
    let text = unsafe { CStr::from_ptr(input) }
        .to_str()
        .map_err(|_| engine::error(error_code::INVALID_MESSAGE))?;
    if text.len() > engine::MAX_INPUT {
        return Err(engine::error(error_code::INVALID_FORMAT));
    }
    Ok(text)
}

unsafe fn write(output: *mut c_char, size: u32, text: &str) {
    if output.is_null() || size == 0 {
        return;
    }
    let mut length = text.len().min(size as usize - 1);
    while !text.is_char_boundary(length) {
        length -= 1;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(text.as_ptr(), output.cast(), length);
        *output.add(length) = 0;
    }
}

#[cfg(any(not(feature = "arma-2.22-no-escape"), test))]
fn normalize_escaped_argument(value: &str) -> String {
    let Some(value) = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    else {
        return value.to_string();
    };
    value.replace("\"\"", "\"")
}

fn normalize_argument(value: &str) -> String {
    #[cfg(feature = "arma-2.22-no-escape")]
    {
        value.to_string()
    }
    #[cfg(not(feature = "arma-2.22-no-escape"))]
    {
        normalize_escaped_argument(value)
    }
}

unsafe fn invoke(output: *mut c_char, size: u32, input: *const c_char) {
    if output.is_null() || size == 0 {
        return;
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        let text = unsafe { read(input) }?;
        Ok::<_, String>(engine::exchange(
            text.to_string(),
            size.saturating_sub(1) as usize,
        ))
    }));
    let text = match result {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => error,
        Err(_) => engine::error(error_code::ERROR),
    };
    unsafe {
        write(output, size, &text);
    }
}

unsafe fn args(
    output: *mut c_char,
    size: u32,
    function: *const c_char,
    argv: *const *const c_char,
    argc: u32,
) -> c_int {
    if output.is_null() || size == 0 {
        return 0;
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        if argc > MAX_ARGS || (argc > 0 && argv.is_null()) {
            return Err(engine::error(error_code::INVALID_FORMAT));
        }
        let function = unsafe { read(function) }?;
        if argc == 0 {
            return Ok(engine::exchange(
                function.to_string(),
                size.saturating_sub(1) as usize,
            ));
        }
        let mut text = function.to_string();
        for index in 0..argc as usize {
            let value = unsafe { read(*argv.add(index)) }?;
            text.push(':');
            text.push_str(&normalize_argument(value));
            if text.len() > engine::MAX_INPUT {
                return Err(engine::error(error_code::INVALID_FORMAT));
            }
        }
        Ok(engine::exchange(text, size.saturating_sub(1) as usize))
    }));
    let (text, code) = match result {
        Ok(Ok(value)) => (value, 0),
        Ok(Err(error)) => (error, 1),
        Err(_) => (engine::error(error_code::ERROR), 1),
    };
    unsafe {
        write(output, size, &text);
    }
    code
}

macro_rules! exports {
    ($abi:literal) => {
        #[unsafe(no_mangle)]
        pub unsafe extern $abi fn RVExtension(output: *mut c_char, output_size: u32, function: *const c_char) {
            unsafe { invoke(output, output_size, function); }
        }
        #[unsafe(no_mangle)]
        pub unsafe extern $abi fn RVExtensionArgs(output: *mut c_char, output_size: u32, function: *const c_char, argv: *const *const c_char, argc: u32) -> c_int {
            unsafe { args(output,output_size,function,argv,argc) }
        }
        #[unsafe(no_mangle)]
        pub unsafe extern $abi fn RVExtensionVersion(output: *mut c_char, output_size: u32) {
            unsafe { write(output,output_size,"extDB3 compatible 1.033"); }
        }
        #[unsafe(no_mangle)]
        pub extern $abi fn RVExtensionRegisterCallback(callback_proc: Option<crate::callback::CallbackProc>) {
            crate::callback::register(callback_proc);
        }
    }
}
#[cfg(windows)]
exports!("system");
#[cfg(not(windows))]
exports!("C");

#[cfg(feature = "arma-2.22-no-escape")]
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static RVExtensionFeatureFlags: u64 = 1 << 3;

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn buffer_sizes_keep_guards_and_nul() {
        for size in 0..40 {
            let mut bytes = [0xA5u8; 64];
            unsafe {
                RVExtensionVersion(bytes.as_mut_ptr().cast(), size);
            }
            assert!(bytes[size as usize..].iter().all(|b| *b == 0xA5));
            if size > 0 {
                assert!(bytes[..size as usize].contains(&0));
            }
        }
    }

    #[test]
    fn extension_version_reports_the_compatibility_target() {
        let mut output = [0 as c_char; 32];
        unsafe { RVExtensionVersion(output.as_mut_ptr(), output.len() as u32) };
        assert_eq!(
            unsafe { CStr::from_ptr(output.as_ptr()) }.to_str().unwrap(),
            "extDB3 compatible 1.033"
        );
    }

    #[test]
    fn normalize_arma_default_string_escaping() {
        assert_eq!(normalize_escaped_argument("\"VERSION\""), "VERSION");
        assert_eq!(
            normalize_escaped_argument("\"He said \"\"go\"\"\""),
            "He said \"go\""
        );
        assert_eq!(normalize_escaped_argument("42"), "42");
        assert_eq!(normalize_escaped_argument("\"\""), "");
    }

    #[cfg(not(feature = "arma-2.22-no-escape"))]
    #[test]
    fn args_use_arma_default_string_escaping() {
        assert_eq!(normalize_argument("\"VERSION\""), "VERSION");
    }

    #[cfg(feature = "arma-2.22-no-escape")]
    #[test]
    fn args_preserve_no_escape_strings() {
        assert_eq!(normalize_argument("\"VERSION\""), "\"VERSION\"");
        assert_eq!(RVExtensionFeatureFlags, 1 << 3);
    }

    #[test]
    fn args_accept_arma_maximum_and_reject_one_more() {
        let function = CString::new("9").unwrap();
        let values: Vec<_> = (0..=MAX_ARGS).map(|_| CString::new("").unwrap()).collect();
        let pointers: Vec<_> = values.iter().map(|value| value.as_ptr()).collect();
        let mut output = [0 as c_char; 128];

        let accepted = unsafe {
            RVExtensionArgs(
                output.as_mut_ptr(),
                output.len() as u32,
                function.as_ptr(),
                pointers.as_ptr(),
                MAX_ARGS,
            )
        };
        assert_eq!(accepted, 0);
        assert_eq!(
            unsafe { CStr::from_ptr(output.as_ptr()) }.to_str().unwrap(),
            engine::error(error_code::INVALID_FORMAT)
        );

        output.fill(0);
        let rejected = unsafe {
            RVExtensionArgs(
                output.as_mut_ptr(),
                output.len() as u32,
                function.as_ptr(),
                pointers.as_ptr(),
                MAX_ARGS + 1,
            )
        };
        assert_eq!(rejected, 1);
        assert_eq!(
            unsafe { CStr::from_ptr(output.as_ptr()) }.to_str().unwrap(),
            engine::error(error_code::INVALID_FORMAT)
        );
    }
}
