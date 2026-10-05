#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcSpeakerBackend {
    LinuxConsoleSpeaker,
    WindowsBeepCompatibility,
    Unsupported,
}

impl PcSpeakerBackend {
    pub fn description(self) -> &'static str {
        match self {
            Self::LinuxConsoleSpeaker => "Linux console PC speaker",
            Self::WindowsBeepCompatibility => {
                "Windows Beep compatibility output (normally routed through the audio device)"
            }
            Self::Unsupported => "unsupported on this platform",
        }
    }
}

pub fn pc_speaker_backend() -> PcSpeakerBackend {
    #[cfg(target_os = "linux")]
    {
        PcSpeakerBackend::LinuxConsoleSpeaker
    }

    #[cfg(windows)]
    {
        PcSpeakerBackend::WindowsBeepCompatibility
    }

    #[cfg(not(any(target_os = "linux", windows)))]
    {
        PcSpeakerBackend::Unsupported
    }
}

pub fn pc_speaker_test(frequency_hz: u32, duration_ms: u32) -> Result<(), String> {
    if !(37..=32_767).contains(&frequency_hz) {
        return Err("PC speaker frequency must be between 37 and 32767 Hz".into());
    }
    if !(1..=5_000).contains(&duration_ms) {
        return Err("PC speaker duration must be between 1 and 5000 ms".into());
    }

    #[cfg(target_os = "linux")]
    {
        linux::tone(frequency_hz, duration_ms)
    }

    #[cfg(windows)]
    {
        windows::tone(frequency_hz, duration_ms)
    }

    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = (frequency_hz, duration_ms);
        Err("PC speaker output is not supported on this platform".into())
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::fs::OpenOptions;
    use std::os::fd::AsRawFd;
    use std::os::raw::{c_int, c_ulong};

    const KDMKTONE: c_ulong = 0x4B30;
    const PIT_FREQUENCY_HZ: u32 = 1_193_180;

    unsafe extern "C" {
        fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
    }

    pub fn tone(frequency_hz: u32, duration_ms: u32) -> Result<(), String> {
        let console = ["/dev/console", "/dev/tty0"]
            .into_iter()
            .find_map(|path| {
                OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(path)
                    .ok()
                    .map(|file| (path, file))
            })
            .ok_or_else(|| {
                "could not open /dev/console or /dev/tty0 for PC speaker access; elevated console permissions may be required"
                    .to_string()
            })?;

        let divisor = (PIT_FREQUENCY_HZ / frequency_hz).clamp(1, u16::MAX as u32);
        let argument = ((divisor & 0xffff) << 16) | (duration_ms & 0xffff);

        let result = unsafe { ioctl(console.1.as_raw_fd(), KDMKTONE, c_ulong::from(argument)) };
        if result < 0 {
            return Err(format!(
                "PC speaker ioctl failed on {}: {}",
                console.0,
                std::io::Error::last_os_error()
            ));
        }

        Ok(())
    }
}

#[cfg(windows)]
mod windows {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        #[link_name = "Beep"]
        fn win_beep(frequency_hz: u32, duration_ms: u32) -> i32;
    }

    pub fn tone(frequency_hz: u32, duration_ms: u32) -> Result<(), String> {
        let result = unsafe { win_beep(frequency_hz, duration_ms) };
        if result == 0 {
            return Err(format!(
                "Windows Beep failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }
}
