// Vendored from Cap (https://github.com/CapSoftware/Cap) at d327051c175aa092c06ad9fc6684298b1b5fed6b: crates/enc-ffmpeg/src/base.rs
// Copyright (c) 2023-present Cap Software, Inc. Used by Klipolo under the AGPL-3.0.
// Klipolo changes: the unit tests removed; `send_frame_to` and `process_eof_to` hand packets to a caller's sink in the encoder's time base (export segments route them to files by pts).

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use ffmpeg::{
    Packet, Rational,
    codec::encoder,
    format::{self},
    frame,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EncodedPacket {
    pub bytes: u64,
    pub key: bool,
}

#[derive(Debug, Default)]
pub struct EncodedPacketStats {
    packets: Mutex<Vec<EncodedPacket>>,
}

impl EncodedPacketStats {
    pub fn record(&self, packet: &Packet) {
        if let Ok(mut packets) = self.packets.lock() {
            packets.push(EncodedPacket {
                bytes: packet.size() as u64,
                key: packet.is_key(),
            });
        }
    }

    pub fn snapshot(&self) -> Vec<EncodedPacket> {
        self.packets
            .lock()
            .map(|packets| packets.clone())
            .unwrap_or_default()
    }
}

pub struct EncoderBase {
    packet: ffmpeg::Packet,
    packet_stats: Option<Arc<EncodedPacketStats>>,
    stream_index: usize,
    first_pts: Option<i64>,
    last_frame_pts: Option<i64>,
    last_written_dts: Option<i64>,
    /// One-packet reorder buffer: a packet is written once its successor is
    /// known so a synthesized duration can be replaced with the real dts
    /// delta. Fragmenting muxers place each fragment at the accumulated
    /// duration of the previous one, and the last sample of a fragment uses
    /// the packet duration verbatim — a nominal duration there collapses any
    /// capture gap that lands on a fragment cut, playing post-gap content
    /// during the gap. The bool records whether the duration was synthesized.
    held_packet: Option<(Packet, bool)>,
}

impl EncoderBase {
    pub(crate) fn new(stream_index: usize) -> Self {
        Self {
            packet: Packet::empty(),
            packet_stats: None,
            first_pts: None,
            last_frame_pts: None,
            stream_index,
            last_written_dts: None,
            held_packet: None,
        }
    }

    pub fn set_packet_stats(&mut self, stats: Arc<EncodedPacketStats>) {
        self.packet_stats = Some(stats);
    }

    pub fn update_pts(
        &mut self,
        frame: &mut frame::Frame,
        timestamp: Duration,
        encoder: &mut encoder::encoder::Encoder,
    ) {
        if timestamp != Duration::MAX {
            let time_base = encoder.time_base();
            let rate = time_base.denominator() as f64 / time_base.numerator() as f64;

            let pts = (timestamp.as_secs_f64() * rate).round() as i64;
            let first_pts = self.first_pts.get_or_insert(pts);

            let pts = normalize_input_pts(pts - *first_pts, self.last_frame_pts);
            self.last_frame_pts = Some(pts);
            frame.set_pts(Some(pts));
        } else {
            let Some(pts) = frame.pts() else {
                tracing::error!("Frame has no pts");
                return;
            };

            let first_pts = self.first_pts.get_or_insert(pts);

            let pts = normalize_input_pts(pts - *first_pts, self.last_frame_pts);
            self.last_frame_pts = Some(pts);
            frame.set_pts(Some(pts));
        }
    }

    /// Stamps the frame's pts from its capture timestamp using an explicit
    /// tick rate. Audio input frames must be stamped in *input sample rate*
    /// units — the resampler rescales them to the encoder's output rate —
    /// whereas [`Self::update_pts`] uses the encoder's own (output) time
    /// base. Mixing the two conventions plays non-48kHz microphones at the
    /// wrong speed.
    pub fn update_pts_with_rate(
        &mut self,
        frame: &mut frame::Frame,
        timestamp: Duration,
        rate: f64,
    ) {
        if timestamp != Duration::MAX {
            let pts = (timestamp.as_secs_f64() * rate).round() as i64;
            let first_pts = *self.first_pts.get_or_insert(pts);
            let mut pts = pts - first_pts;
            if let Some(last) = self.last_frame_pts
                && pts <= last
            {
                pts = last + 1;
            }
            self.last_frame_pts = Some(pts);
            frame.set_pts(Some(pts));
        } else if let Some(pts) = frame.pts() {
            let first_pts = *self.first_pts.get_or_insert(pts);
            let mut pts = pts - first_pts;
            if let Some(last) = self.last_frame_pts
                && pts <= last
            {
                pts = last + 1;
            }
            self.last_frame_pts = Some(pts);
            frame.set_pts(Some(pts));
        } else {
            tracing::error!("Frame has no pts");
        }
    }

    pub fn send_frame(
        &mut self,
        frame: &frame::Frame,
        output: &mut format::context::Output,
        encoder: &mut encoder::encoder::Encoder,
    ) -> Result<(), ffmpeg::Error> {
        encoder.send_frame(frame)?;

        self.process_packets(output, encoder)
    }

    /// Klipolo: sends `frame` and hands every packet that is ready to `sink`, in the encoder's time base with its pts and
    /// dts set (the packet is reused: the sink writes or copies it).
    pub fn send_frame_to(
        &mut self,
        frame: &frame::Frame,
        encoder: &mut encoder::encoder::Encoder,
        sink: &mut dyn FnMut(&mut Packet) -> Result<(), ffmpeg::Error>,
    ) -> Result<(), ffmpeg::Error> {
        encoder.send_frame(frame)?;
        self.drain_to(encoder, sink)
    }

    /// Klipolo: ends the stream and hands the remaining packets to `sink` (see `send_frame_to`).
    pub fn process_eof_to(
        &mut self,
        encoder: &mut encoder::encoder::Encoder,
        sink: &mut dyn FnMut(&mut Packet) -> Result<(), ffmpeg::Error>,
    ) -> Result<(), ffmpeg::Error> {
        encoder.send_eof()?;
        self.drain_to(encoder, sink)
    }

    fn drain_to(
        &mut self,
        encoder: &mut encoder::encoder::Encoder,
        sink: &mut dyn FnMut(&mut Packet) -> Result<(), ffmpeg::Error>,
    ) -> Result<(), ffmpeg::Error> {
        while packet_available(encoder.receive_packet(&mut self.packet))? {
            match (self.packet.pts(), self.packet.dts()) {
                (Some(pts), None) => self.packet.set_dts(Some(pts)),
                (None, Some(dts)) => self.packet.set_pts(Some(dts)),
                _ => {}
            }
            sink(&mut self.packet)?;
        }
        Ok(())
    }

    fn process_packets(
        &mut self,
        output: &mut format::context::Output,
        encoder: &mut encoder::encoder::Encoder,
    ) -> Result<(), ffmpeg::Error> {
        while packet_available(encoder.receive_packet(&mut self.packet))? {
            self.packet.set_stream(self.stream_index);
            self.packet.rescale_ts(
                encoder.time_base(),
                output.stream(self.stream_index).unwrap().time_base(),
            );

            match (self.packet.pts(), self.packet.dts()) {
                (Some(pts), None) => self.packet.set_dts(Some(pts)),
                (None, Some(dts)) => self.packet.set_pts(Some(dts)),
                _ => {}
            }

            let duration_synthesized = self.packet.duration() <= 0;
            if duration_synthesized
                && let Some(duration) = nominal_packet_duration(
                    output.stream(self.stream_index).unwrap().time_base(),
                    encoder.frame_rate(),
                )
            {
                self.packet.set_duration(duration);
            }

            if let (Some(dts), Some(last_dts)) = (self.packet.dts(), self.last_written_dts)
                && dts <= last_dts
            {
                let fixed_dts = last_dts + 1;
                self.packet.set_dts(Some(fixed_dts));
                if let Some(pts) = self.packet.pts()
                    && pts < fixed_dts
                {
                    self.packet.set_pts(Some(fixed_dts));
                }
            }

            if let (Some(pts), Some(dts)) = (self.packet.pts(), self.packet.dts())
                && pts < dts
            {
                self.packet.set_pts(Some(dts));
            }

            self.last_written_dts = self.packet.dts();
            if let Some(stats) = &self.packet_stats {
                stats.record(&self.packet);
            }

            let current = std::mem::replace(&mut self.packet, Packet::empty());
            if let Some((mut previous, previous_synthesized)) = self.held_packet.take() {
                if previous_synthesized
                    && let (Some(prev_dts), Some(cur_dts)) = (previous.dts(), current.dts())
                    && cur_dts > prev_dts
                {
                    previous.set_duration(cur_dts - prev_dts);
                }
                previous.write_interleaved(output)?;
            }
            self.held_packet = Some((current, duration_synthesized));
        }

        Ok(())
    }

    pub fn process_eof(
        &mut self,
        output: &mut format::context::Output,
        encoder: &mut encoder::encoder::Encoder,
    ) -> Result<(), ffmpeg::Error> {
        encoder.send_eof()?;

        self.process_packets(output, encoder)?;

        if let Some((previous, _)) = self.held_packet.take() {
            previous.write_interleaved(output)?;
        }

        Ok(())
    }
}

fn packet_available(result: Result<(), ffmpeg::Error>) -> Result<bool, ffmpeg::Error> {
    match result {
        Ok(()) => Ok(true),
        Err(ffmpeg::Error::Eof) => Ok(false),
        Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => Ok(false),
        Err(error) => Err(error),
    }
}

fn nominal_packet_duration(time_base: Rational, frame_rate: Rational) -> Option<i64> {
    let time_base_num = time_base.numerator();
    let time_base_den = time_base.denominator();
    let frame_rate_num = frame_rate.numerator();
    let frame_rate_den = frame_rate.denominator();

    if time_base_num <= 0 || time_base_den <= 0 || frame_rate_num <= 0 || frame_rate_den <= 0 {
        return None;
    }

    let ticks = (frame_rate_den as f64 * time_base_den as f64)
        / (frame_rate_num as f64 * time_base_num as f64);
    ticks
        .is_finite()
        .then(|| ticks.round() as i64)
        .filter(|ticks| *ticks > 0)
}

fn normalize_input_pts(pts: i64, last_pts: Option<i64>) -> i64 {
    let Some(last_pts) = last_pts else {
        return pts;
    };

    if pts > last_pts {
        return pts;
    }

    // A tie or backwards timestamp carries no time — advance a single tick
    // so pts stay strictly monotonic and the real timestamps immediately
    // take over again. Any larger bump outruns a source delivering faster
    // than the nominal rate (each corrected frame pushes the next one into
    // correction too), re-timing the recording to the bump cadence.
    last_pts + 1
}
