use std::ffi::CStr;

/// The FFmpeg build this process actually loaded (the release scanner and `--export-worker --version` print it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FfmpegInfo {
    pub version: String,
    pub license: String,
}

pub fn ffmpeg_info() -> FfmpegInfo {
    // SAFETY: both functions return pointers to static, NUL-terminated strings owned by libavutil/libavcodec.
    unsafe {
        FfmpegInfo {
            version: CStr::from_ptr(ffmpeg::ffi::av_version_info())
                .to_string_lossy()
                .into_owned(),
            license: CStr::from_ptr(ffmpeg::ffi::avcodec_license())
                .to_string_lossy()
                .into_owned(),
        }
    }
}
