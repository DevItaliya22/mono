// SPDX-License-Identifier: MPL-2.0

//! UIA physical bounds and native virtual-screen metrics must share DPI context.
use std::{marker::PhantomData, rc::Rc};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
};

pub struct PhysicalCoordinates {
    previous: DPI_AWARENESS_CONTEXT,
    _thread: PhantomData<Rc<()>>,
}
impl PhysicalCoordinates {
    pub fn enter() -> Result<Self, ()> {
        let previous =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        if previous.0.is_null() {
            return Err(());
        }
        Ok(Self {
            previous,
            _thread: PhantomData,
        })
    }
}
impl Drop for PhysicalCoordinates {
    fn drop(&mut self) {
        unsafe { SetThreadDpiAwarenessContext(self.previous) };
    }
}
