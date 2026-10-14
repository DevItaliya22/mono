// SPDX-License-Identifier: MPL-2.0

//! A UIA client whose COM apartment outlives every retained native target.
use std::rc::Rc;

use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::UI::Accessibility::{CUIAutomation, CUIAutomation8, IUIAutomation};

pub(crate) struct ComApartment;

impl ComApartment {
    fn initialize() -> windows::core::Result<Rc<Self>> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok()?;
        Ok(Rc::new(Self))
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

/// Construct, use and drop on one worker. The Rc makes this !Send and !Sync.
/// Fields drop in declaration order: the UIA interface precedes COM teardown.
pub struct AutomationClient {
    pub(crate) automation: IUIAutomation,
    pub(crate) apartment: Rc<ComApartment>,
}

impl AutomationClient {
    /// Preserve the Agent's existing CUIAutomation provider choice.
    pub fn open() -> windows::core::Result<Self> {
        Self::create(&CUIAutomation)
    }

    /// Preserve the runner's existing CUIAutomation8 provider choice.
    pub fn open_v8() -> windows::core::Result<Self> {
        Self::create(&CUIAutomation8)
    }

    fn create(class: &windows::core::GUID) -> windows::core::Result<Self> {
        let apartment = ComApartment::initialize()?;
        let automation = unsafe { CoCreateInstance(class, None, CLSCTX_INPROC_SERVER) }?;
        Ok(Self {
            automation,
            apartment,
        })
    }

    /// Borrow on the owning thread for caller-owned tree traversal.
    /// Retained interfaces must be released before this client.
    pub fn automation(&self) -> &IUIAutomation {
        &self.automation
    }
}
