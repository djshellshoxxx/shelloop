pub fn sanitize_sample(sample: f32) -> f32 {
    if sample.is_finite() {
        sample.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

pub fn create_sample_renderer<F, R>(sample_rate: u32, factory: F) -> Result<R, String>
where
    F: FnOnce(u32) -> Result<R, String>,
{
    if sample_rate == 0 {
        return Err("audio sample rate must be greater than zero".into());
    }
    factory(sample_rate)
}

pub fn write_mono_interleaved(
    output: &mut [f32],
    channels: usize,
    mono: &[f32],
) -> Result<usize, String> {
    if channels == 0 {
        return Err("channel count must be greater than zero".into());
    }

    let required = mono
        .len()
        .checked_mul(channels)
        .ok_or_else(|| "audio buffer size overflow".to_string())?;
    if output.len() < required {
        return Err(format!(
            "output buffer too short: need {required} samples, got {}",
            output.len()
        ));
    }

    for (frame, sample) in output[..required]
        .chunks_exact_mut(channels)
        .zip(mono.iter().copied())
    {
        let sample = sanitize_sample(sample);
        frame.fill(sample);
    }

    Ok(mono.len())
}

pub fn write_stereo_interleaved(
    output: &mut [f32],
    channels: usize,
    stereo: &[(f32, f32)],
) -> Result<usize, String> {
    if channels == 0 {
        return Err("channel count must be greater than zero".into());
    }

    let required = stereo
        .len()
        .checked_mul(channels)
        .ok_or_else(|| "audio buffer size overflow".to_string())?;
    if output.len() < required {
        return Err(format!(
            "output buffer too short: need {required} samples, got {}",
            output.len()
        ));
    }

    for (frame, (left, right)) in output[..required]
        .chunks_exact_mut(channels)
        .zip(stereo.iter().copied())
    {
        let left = sanitize_sample(left);
        let right = sanitize_sample(right);
        if channels == 1 {
            frame[0] = sanitize_sample((left + right) * 0.5);
        } else {
            frame[0] = left;
            frame[1] = right;
            for extra in &mut frame[2..] {
                *extra = 0.0;
            }
        }
    }

    Ok(stereo.len())
}

pub fn select_named_device_index(names: &[String], requested: &str) -> Result<usize, String> {
    let requested = requested.trim();
    if requested.is_empty() {
        return Err("audio device name may not be empty".into());
    }

    names
        .iter()
        .position(|name| name.eq_ignore_ascii_case(requested))
        .ok_or_else(|| format!("audio output device not found: {requested}"))
}

#[cfg(feature = "realtime-audio")]
mod realtime {
    use super::{create_sample_renderer, sanitize_sample};
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{FromSample, SampleFormat, SizedSample};
    use crossbeam_channel::{bounded, Receiver, Sender};

    pub struct AudioOutput {
        _stream: cpal::Stream,
        device_name: String,
        sample_rate: u32,
        channels: u16,
        errors: Receiver<String>,
    }

    impl AudioOutput {
        pub fn device_name(&self) -> &str {
            &self.device_name
        }

        pub fn sample_rate(&self) -> u32 {
            self.sample_rate
        }

        pub fn channels(&self) -> u16 {
            self.channels
        }

        pub fn take_error(&self) -> Option<String> {
            self.errors.try_recv().ok()
        }
    }

    pub fn list_output_device_names() -> Result<Vec<String>, String> {
        let host = cpal::default_host();
        let devices = host
            .output_devices()
            .map_err(|error| format!("failed to enumerate audio output devices: {error}"))?;

        let mut names = Vec::new();
        for device in devices {
            let name = device
                .name()
                .map_err(|error| format!("failed to read audio device name: {error}"))?;
            names.push(name);
        }
        Ok(names)
    }

    pub fn open_output_stream<F, R>(
        requested_device: Option<&str>,
        renderer_factory: F,
    ) -> Result<AudioOutput, String>
    where
        F: FnOnce(u32) -> Result<R, String>,
        R: FnMut() -> f32 + Send + 'static,
    {
        let host = cpal::default_host();
        let device = if let Some(requested) = requested_device {
            let requested = requested.trim();
            if requested.is_empty() {
                return Err("audio device name may not be empty".into());
            }

            let mut matched = None;
            let devices = host
                .output_devices()
                .map_err(|error| format!("failed to enumerate audio output devices: {error}"))?;
            for device in devices {
                let name = device
                    .name()
                    .map_err(|error| format!("failed to read audio device name: {error}"))?;
                if name.eq_ignore_ascii_case(requested) {
                    matched = Some(device);
                    break;
                }
            }
            matched.ok_or_else(|| format!("audio output device not found: {requested}"))?
        } else {
            host.default_output_device()
                .ok_or_else(|| "no default audio output device is available".to_string())?
        };

        let device_name = device
            .name()
            .unwrap_or_else(|_| "unknown output device".to_string());
        let supported = device
            .default_output_config()
            .map_err(|error| format!("failed to query default output configuration: {error}"))?;
        let sample_format = supported.sample_format();
        let sample_rate = supported.sample_rate().0;
        let config = supported.config();
        let channels = config.channels;
        if channels == 0 {
            return Err("audio output device reported zero channels".into());
        }

        let next_sample = create_sample_renderer(sample_rate, renderer_factory)?;
        let (error_sender, error_receiver) = bounded(32);
        let stream = match sample_format {
            SampleFormat::F32 => {
                build_stream::<f32, _>(&device, &config, next_sample, error_sender)
            }
            SampleFormat::I16 => {
                build_stream::<i16, _>(&device, &config, next_sample, error_sender)
            }
            SampleFormat::U16 => {
                build_stream::<u16, _>(&device, &config, next_sample, error_sender)
            }
            other => Err(format!("unsupported audio sample format: {other}")),
        }?;

        stream
            .play()
            .map_err(|error| format!("failed to start audio output stream: {error}"))?;

        Ok(AudioOutput {
            _stream: stream,
            device_name,
            sample_rate,
            channels,
            errors: error_receiver,
        })
    }

    pub fn open_stereo_output_stream<F, R>(
        requested_device: Option<&str>,
        renderer_factory: F,
    ) -> Result<AudioOutput, String>
    where
        F: FnOnce(u32) -> Result<R, String>,
        R: FnMut() -> (f32, f32) + Send + 'static,
    {
        let host = cpal::default_host();
        let device = if let Some(requested) = requested_device {
            let requested = requested.trim();
            if requested.is_empty() {
                return Err("audio device name may not be empty".into());
            }

            let mut matched = None;
            let devices = host
                .output_devices()
                .map_err(|error| format!("failed to enumerate audio output devices: {error}"))?;
            for device in devices {
                let name = device
                    .name()
                    .map_err(|error| format!("failed to read audio device name: {error}"))?;
                if name.eq_ignore_ascii_case(requested) {
                    matched = Some(device);
                    break;
                }
            }
            matched.ok_or_else(|| format!("audio output device not found: {requested}"))?
        } else {
            host.default_output_device()
                .ok_or_else(|| "no default audio output device is available".to_string())?
        };

        let device_name = device
            .name()
            .unwrap_or_else(|_| "unknown output device".to_string());
        let supported = device
            .default_output_config()
            .map_err(|error| format!("failed to query default output configuration: {error}"))?;
        let sample_format = supported.sample_format();
        let sample_rate = supported.sample_rate().0;
        let config = supported.config();
        let channels = config.channels;
        if channels == 0 {
            return Err("audio output device reported zero channels".into());
        }

        let next_frame = create_sample_renderer(sample_rate, renderer_factory)?;
        let (error_sender, error_receiver) = bounded(32);
        let stream = match sample_format {
            SampleFormat::F32 => {
                build_stereo_stream::<f32, _>(&device, &config, next_frame, error_sender)
            }
            SampleFormat::I16 => {
                build_stereo_stream::<i16, _>(&device, &config, next_frame, error_sender)
            }
            SampleFormat::U16 => {
                build_stereo_stream::<u16, _>(&device, &config, next_frame, error_sender)
            }
            other => Err(format!("unsupported audio sample format: {other}")),
        }?;

        stream
            .play()
            .map_err(|error| format!("failed to start audio output stream: {error}"))?;

        Ok(AudioOutput {
            _stream: stream,
            device_name,
            sample_rate,
            channels,
            errors: error_receiver,
        })
    }

    fn build_stereo_stream<T, F>(
        device: &cpal::Device,
        config: &cpal::StreamConfig,
        mut next_frame: F,
        error_sender: Sender<String>,
    ) -> Result<cpal::Stream, String>
    where
        T: SizedSample + FromSample<f32>,
        F: FnMut() -> (f32, f32) + Send + 'static,
    {
        let channels = usize::from(config.channels);
        device
            .build_output_stream(
                config,
                move |data: &mut [T], _| {
                    for frame in data.chunks_mut(channels) {
                        let (left, right) = next_frame();
                        let left = sanitize_sample(left);
                        let right = sanitize_sample(right);
                        if channels == 1 {
                            frame[0] = T::from_sample(sanitize_sample((left + right) * 0.5));
                        } else {
                            frame[0] = T::from_sample(left);
                            frame[1] = T::from_sample(right);
                            for output in &mut frame[2..] {
                                *output = T::from_sample(0.0);
                            }
                        }
                    }
                },
                move |error| {
                    let _ = error_sender.try_send(error.to_string());
                },
                None,
            )
            .map_err(|error| format!("failed to build stereo audio output stream: {error}"))
    }

    fn build_stream<T, F>(
        device: &cpal::Device,
        config: &cpal::StreamConfig,
        mut next_sample: F,
        error_sender: Sender<String>,
    ) -> Result<cpal::Stream, String>
    where
        T: SizedSample + FromSample<f32>,
        F: FnMut() -> f32 + Send + 'static,
    {
        let channels = usize::from(config.channels);
        device
            .build_output_stream(
                config,
                move |data: &mut [T], _| {
                    for frame in data.chunks_mut(channels) {
                        let sample = sanitize_sample(next_sample());
                        for output in frame {
                            *output = T::from_sample(sample);
                        }
                    }
                },
                move |error| {
                    let _ = error_sender.try_send(error.to_string());
                },
                None,
            )
            .map_err(|error| format!("failed to build audio output stream: {error}"))
    }
}

#[cfg(feature = "realtime-audio")]
pub use realtime::{
    list_output_device_names, open_output_stream, open_stereo_output_stream, AudioOutput,
};
