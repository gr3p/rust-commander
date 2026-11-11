use std::ffi::{CString, c_void};

/// A shared library loaded at run time and kept for the life of the process.
pub struct Lib(*mut c_void);

// The handle is only a key for looking up symbols, and the loader is thread safe.
unsafe impl Send for Lib {}
unsafe impl Sync for Lib {}

impl Lib {
    /// The first of `names` the loader finds.
    pub fn open(names: &[&str]) -> Option<Lib> {
        names.iter().find_map(|name| {
            let name = CString::new(*name).ok()?;
            let handle = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
            (!handle.is_null()).then_some(Lib(handle))
        })
    }

    /// A function from the library. `T` must be the `extern "C"` function pointer type of the symbol.
    pub fn symbol<T: Copy>(&self, name: &str) -> Result<T, String> {
        if std::mem::size_of::<T>() != std::mem::size_of::<*mut c_void>() {
            return Err(format!("{name} is not a function pointer type"));
        }
        let cname = CString::new(name).map_err(|_| format!("bad symbol name {name}"))?;
        let ptr = unsafe { libc::dlsym(self.0, cname.as_ptr()) };
        if ptr.is_null() {
            return Err(format!("The library has no {name}"));
        }
        // SAFETY: the sizes match, and the caller names the type the symbol really has.
        Ok(unsafe { std::mem::transmute_copy::<*mut c_void, T>(&ptr) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_symbol_in_libc_and_rejects_the_rest() {
        let lib = Lib::open(&["libc.so.6", "libc.so"]).expect("libc loads");
        let getpid: unsafe extern "C" fn() -> i32 = lib.symbol("getpid").unwrap();
        assert_eq!(unsafe { getpid() }, std::process::id() as i32);
        assert!(lib.symbol::<unsafe extern "C" fn()>("no_such_function_xyz").is_err());
        assert!(lib.symbol::<u8>("getpid").is_err());
        assert!(Lib::open(&["libdoes-not-exist.so.9"]).is_none());
    }

    #[test]
    fn falls_back_to_the_next_name() {
        assert!(Lib::open(&["libdoes-not-exist.so.9", "libc.so.6"]).is_some());
    }
}
