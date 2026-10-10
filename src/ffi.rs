use crate::engine;
use std::{
    ffi::{CStr, c_char, c_int},
    panic::{AssertUnwindSafe, catch_unwind},
};

unsafe fn read<'a>(input: *const c_char) -> Result<&'a str, String> {
    if input.is_null() {
        return Err(engine::error("Error Invalid Message"));
    }
    let text = unsafe { CStr::from_ptr(input) }
        .to_str()
        .map_err(|_| engine::error("Error Invalid Encoding"))?;
    if text.len() > engine::MAX_INPUT {
        return Err(engine::error("Error Input Limit"));
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
        Err(_) => engine::error("Error Internal"),
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
        if argc > 256 || (argc > 0 && argv.is_null()) {
            return Err(engine::error("Error Invalid Arguments"));
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
            text.push_str(value);
            if text.len() > engine::MAX_INPUT {
                return Err(engine::error("Error Input Limit"));
            }
        }
        Ok(engine::exchange(text, size.saturating_sub(1) as usize))
    }));
    let (text, code) = match result {
        Ok(Ok(value)) => (value, 0),
        Ok(Err(error)) => (error, 1),
        Err(_) => (engine::error("Error Internal"), 1),
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
            unsafe { write(output,output_size,"extDB3 compatible Rust 0.1.0"); }
        }
    }
}
#[cfg(windows)]
exports!("system");
#[cfg(not(windows))]
exports!("C");

#[cfg(test)]
mod tests {
    use super::*;
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
}
