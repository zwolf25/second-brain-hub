//! macOS-only: register/query this app as a Launch Services role handler for
//! the markdown UTI, via the plain C LaunchServices API (no ObjC bridge crate
//! needed for two functions). Windows has no equivalent (Microsoft blocks
//! programmatic default-app changes since Windows 8) — see the frontend's
//! guided Settings link instead.

use core_foundation::base::TCFType;
use core_foundation::string::{CFString, CFStringRef};

const BUNDLE_ID: &str = "com.zackwolf.second-brain-search";
const MARKDOWN_UTI: &str = "net.daringfireball.markdown";
const K_LS_ROLES_VIEWER: u32 = 0x0000_0002;

#[link(name = "CoreServices", kind = "framework")]
extern "C" {
    fn LSSetDefaultRoleHandlerForContentType(
        in_content_type: CFStringRef,
        in_role: u32,
        in_handler_bundle_id: CFStringRef,
    ) -> i32;

    fn LSCopyDefaultRoleHandlerForContentType(in_content_type: CFStringRef, in_role: u32) -> CFStringRef;
}

pub fn set_as_default() -> Result<(), String> {
    let uti = CFString::new(MARKDOWN_UTI);
    let bundle_id = CFString::new(BUNDLE_ID);
    let status = unsafe {
        LSSetDefaultRoleHandlerForContentType(
            uti.as_concrete_TypeRef(),
            K_LS_ROLES_VIEWER,
            bundle_id.as_concrete_TypeRef(),
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err(format!("LSSetDefaultRoleHandlerForContentType failed: OSStatus {status}"))
    }
}

pub fn is_default() -> bool {
    let uti = CFString::new(MARKDOWN_UTI);
    let current: CFStringRef =
        unsafe { LSCopyDefaultRoleHandlerForContentType(uti.as_concrete_TypeRef(), K_LS_ROLES_VIEWER) };
    if current.is_null() {
        return false;
    }
    let current = unsafe { CFString::wrap_under_create_rule(current) };
    current.to_string() == BUNDLE_ID
}

#[cfg(test)]
mod spike {
    use super::*;

    // Not run by normal `cargo test` — reads real system Launch Services
    // state, which varies by machine. Run explicitly:
    // `cargo test -- --ignored current_default_handler --nocapture`
    #[test]
    #[ignore]
    fn current_default_handler() {
        let uti = CFString::new(MARKDOWN_UTI);
        let current: CFStringRef =
            unsafe { LSCopyDefaultRoleHandlerForContentType(uti.as_concrete_TypeRef(), K_LS_ROLES_VIEWER) };
        if current.is_null() {
            println!("no default handler registered for {MARKDOWN_UTI}");
        } else {
            let current = unsafe { CFString::wrap_under_create_rule(current) };
            println!("current default handler for {MARKDOWN_UTI}: {current}");
        }
        println!("this app's bundle id: {BUNDLE_ID}");
        println!("is_default() as seen by this binary: {}", is_default());
    }
}
