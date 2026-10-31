//! Decoding with Klipolo's own FFmpeg (the LGPL build the engine links), to check written files frame for frame (video)
//! and sample for sample (audio).

/// Decodes every frame of `path` with Klipolo's FFmpeg; returns (frames, last pts in µs).
pub fn decode_all(path: &std::path::Path) -> (u64, i64) {
    ffmpeg::init().unwrap();
    let mut input = ffmpeg::format::input(path).unwrap();
    let stream = input.streams().best(ffmpeg::media::Type::Video).unwrap();
    let index = stream.index();
    let tb = stream.time_base();
    let mut decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
        .unwrap()
        .decoder()
        .video()
        .unwrap();
    let (mut frames, mut last) = (0u64, 0i64);
    let mut frame = ffmpeg::frame::Video::empty();
    let mut drain = |d: &mut ffmpeg::decoder::Video, frames: &mut u64, last: &mut i64| {
        while d.receive_frame(&mut frame).is_ok() {
            *frames += 1;
            *last = frame.pts().unwrap_or(0) * 1_000_000 * tb.numerator() as i64
                / tb.denominator() as i64;
        }
    };
    for (s, packet) in input.packets() {
        if s.index() == index {
            decoder.send_packet(&packet).unwrap();
            drain(&mut decoder, &mut frames, &mut last);
        }
    }
    decoder.send_eof().unwrap();
    drain(&mut decoder, &mut frames, &mut last);
    (frames, last)
}

/// Decodes every audio frame of `path` with Klipolo's FFmpeg; returns the frames (samples per channel).
pub fn decode_audio_frames(path: &std::path::Path) -> u64 {
    ffmpeg::init().unwrap();
    let mut input = ffmpeg::format::input(path).unwrap();
    let stream = input.streams().best(ffmpeg::media::Type::Audio).unwrap();
    let index = stream.index();
    let mut decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
        .unwrap()
        .decoder()
        .audio()
        .unwrap();
    let mut frames = 0u64;
    let mut frame = ffmpeg::frame::Audio::empty();
    let mut drain = |d: &mut ffmpeg::decoder::Audio, frames: &mut u64| {
        while d.receive_frame(&mut frame).is_ok() {
            *frames += frame.samples() as u64;
        }
    };
    for (s, packet) in input.packets() {
        if s.index() == index {
            decoder.send_packet(&packet).unwrap();
            drain(&mut decoder, &mut frames);
        }
    }
    decoder.send_eof().unwrap();
    drain(&mut decoder, &mut frames);
    frames
}
