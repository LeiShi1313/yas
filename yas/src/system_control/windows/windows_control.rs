use enigo::{Enigo, MouseButton, MouseControllable};
use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    mouse_event, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, SetCursorPos};

pub struct WindowsSystemControl {
    enigo: Enigo,
}

impl WindowsSystemControl {
    pub fn new() -> WindowsSystemControl {
        WindowsSystemControl {
            enigo: Enigo::new(),
        }
    }

    pub fn mouse_move_to(&mut self, x: i32, y: i32) -> anyhow::Result<()> {
        let result = unsafe { SetCursorPos(x, y) };
        if result == 0 {
            let mut position = POINT { x: 0, y: 0 };
            let verified =
                unsafe { GetCursorPos(&mut position) } != 0 && position.x == x && position.y == y;
            if !verified {
                return Err(std::io::Error::last_os_error().into());
            }
        }

        anyhow::Ok(())
    }

    pub fn mouse_click(&mut self) -> anyhow::Result<()> {
        self.enigo.mouse_click(MouseButton::Left);

        anyhow::Ok(())
    }

    pub fn mouse_down(&mut self) -> anyhow::Result<()> {
        unsafe { mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0) };

        anyhow::Ok(())
    }

    pub fn mouse_up(&mut self) -> anyhow::Result<()> {
        unsafe { mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0) };

        anyhow::Ok(())
    }

    pub fn mouse_scroll(&mut self, amount: i32, _try_find: bool) -> anyhow::Result<()> {
        self.enigo.mouse_scroll_y(amount);

        anyhow::Ok(())
    }
}
