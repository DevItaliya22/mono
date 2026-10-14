// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn pinned_binding_null_interface_ends_a_branch_without_hiding_provider_failure() {
    // Exercise the exact conversion the generated nullable UIA getters use.
    // No COM initialization, window or provider is involved.
    let null: windows::core::Result<IUIAutomationElement> =
        unsafe { <IUIAutomationElement as windows::core::Type<_>>::from_abi(std::ptr::null_mut()) };
    assert_eq!(null.as_ref().unwrap_err().code(), windows::core::HRESULT(0));
    assert!(matches!(optional(null), Ok(None)));

    for code in [0x80004003_u32, 0x80004005, 0x80040201] {
        let error = windows::core::Error::from_hresult(windows::core::HRESULT(code as i32));
        assert!(
            optional(Err(error)).is_err(),
            "real provider HRESULT must not end traversal"
        );
    }
}
