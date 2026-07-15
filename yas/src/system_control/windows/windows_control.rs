use anyhow::bail;
use enigo::{Enigo, MouseButton, MouseControllable};
use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    mouse_event, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    KEYEVENTF_SCANCODE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, SetCursorPos};

pub struct WindowsSystemControl {
    enigo: Enigo,
}

impl WindowsSystemControl {
    const SCAN_ALT: u8 = 0x38;
    const SCAN_B: u8 = 0x30;
    const SCAN_E: u8 = 0x12;
    const SCAN_ENTER: u8 = 0x1c;
    const SCAN_ESCAPE: u8 = 0x01;
    const SCAN_Q: u8 = 0x10;

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

    pub fn key_click(&mut self, key: char) -> anyhow::Result<()> {
        let scan_code = match key.to_ascii_lowercase() {
            'b' => Self::SCAN_B,
            'e' => Self::SCAN_E,
            'q' => Self::SCAN_Q,
            unsupported => bail!("unsupported scan-code key: {unsupported}"),
        };
        Self::send_scan_code(scan_code, KEYEVENTF_SCANCODE)?;
        std::thread::sleep(std::time::Duration::from_millis(30));
        Self::send_scan_code(scan_code, KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP)
    }

    pub fn key_escape(&mut self) -> anyhow::Result<()> {
        Self::send_scan_code(Self::SCAN_ESCAPE, KEYEVENTF_SCANCODE)?;
        std::thread::sleep(std::time::Duration::from_millis(30));
        Self::send_scan_code(Self::SCAN_ESCAPE, KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP)
    }

    pub fn key_alt_enter(&mut self) -> anyhow::Result<()> {
        Self::send_scan_code(Self::SCAN_ALT, KEYEVENTF_SCANCODE)?;
        std::thread::sleep(std::time::Duration::from_millis(30));
        Self::send_scan_code(Self::SCAN_ENTER, KEYEVENTF_SCANCODE)?;
        std::thread::sleep(std::time::Duration::from_millis(30));
        Self::send_scan_code(Self::SCAN_ENTER, KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP)?;
        Self::send_scan_code(Self::SCAN_ALT, KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP)
    }

    fn send_scan_code(scan_code: u8, flags: u32) -> anyhow::Result<()> {
        let input = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: 0,
                    wScan: scan_code as u16,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let sent = unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32) };
        if sent != 1 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }
}
