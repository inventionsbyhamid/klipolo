//! The one way Klipolo opens a media file for reading through libavformat. A file's
//! contents choose its demuxer, and some demuxers read other files by the names inside it: an `ffconcat` list, an
//! image named with a number pattern (`x%d.png` reads `x0.png`…), a playlist, a subfile, a network session. A crafted
//! file named like media would then make the probe, and every decode after it, read whatever those names reach (links
//! included), so every input opens here, with:
//! - `format_whitelist`: only the demuxers for what Klipolo reads. The person's import types (`mp4 mov m4v mkv webm avi
//!   mp3 wav m4a aac flac ogg png jpg jpeg webp heic avif tif tiff gif`, the import's extension list) and Klipolo's own
//!   files (recordings' `mp4` and `caf`, editing copies, proxies and export segments in `mp4`/`mov`) need `mov` (HEIC
//!   and AVIF too), `matroska`, `avi`, `mp3`, `wav`, `aac`, `flac`, `ogg`, `caf`, `gif`, `apng` (an animated PNG: it
//!   wins over `png_pipe` for one), the single-image pipes (`tiff_pipe` for a TIFF) and `image2`. A file whose
//!   contents pick any other demuxer (`concat`, `hls`, `subfile`, `sdp`, `rtsp`, `tty`…) is refused before that
//!   demuxer reads anything;
//! - `pattern_type=none`: `image2` (which wins for most JPEG photos, by their extension) reads the file named, never a
//!   numbered sequence or a glob;
//! - `protocol_whitelist=file`: nothing reaches the network or another protocol;
//! - `enable_drefs=0`: `mov` never opens the external data references a QuickTime file can name (FFmpeg's default,
//!   stated so that no caller's options can turn it on);
//! - `ignore_loop=1`: `apng` reads an endlessly looping file's frames once (its default, stated the same way).
//!
//! The guard's options are set after a caller's `extra`, so `extra` can't loosen any of them.
//! Test code may open files directly; `tests/media_open.rs` fails on any other direct open. That scan reads Rust
//! only: a C, ObjC or Swift file calling `avformat_open_input`, or a filter graph built from project or
//! agent text with a filter that opens inputs by name (`movie`, `amovie`, `subtitles`), is reviewed by hand. Today
//! there is neither (the one filter graph's chains are `&'static str`).

use std::path::Path;

use ffmpeg::format::context::Input;

/// The demuxers Klipolo reads with (see the module's doc). A demuxer's name lists its aliases (`mov,mp4,m4a,3gp,…`);
/// one of them matching is enough. Widening this: rename the failure mark in `media/activity/files.rs` (and on an FFmpeg
/// upgrade that adds a decoder), so files refused before are analysed again. `tiff_pipe` reads a still picture only,
/// which that pass never analyses, so it came in without a rename.
pub const DEMUXERS: &str = concat!(
    "mov,matroska,avi,mp3,wav,aac,flac,ogg,caf,gif,apng,",
    "image2,png_pipe,jpeg_pipe,webp_pipe,tiff_pipe"
);

/// The options every input opens with, then `extra`'s.
fn guarded(extra: ffmpeg::Dictionary) -> ffmpeg::Dictionary<'static> {
    let mut options = ffmpeg::Dictionary::new();
    for (key, value) in extra.iter() {
        options.set(key, value);
    }
    options.set("format_whitelist", DEMUXERS);
    options.set("protocol_whitelist", "file");
    options.set("pattern_type", "none");
    options.set("enable_drefs", "0");
    options.set("ignore_loop", "1");
    options
}

/// `path` opened for reading (as `ffmpeg::format::input`, its streams found), only through the demuxers Klipolo reads
/// with, from the file alone.
pub fn input<P: AsRef<Path> + ?Sized>(path: &P) -> Result<Input, ffmpeg::Error> {
    let _ = ffmpeg::init();
    ffmpeg::format::input_with_dictionary(path, guarded(ffmpeg::Dictionary::new()))
}

/// The same, given up once `interrupt` says so, with `extra` options besides the guard's (which `extra` can't loosen).
pub fn input_interruptible<P, F>(
    path: &P,
    interrupt: F,
    extra: ffmpeg::Dictionary,
) -> Result<Input, ffmpeg::Error>
where
    P: AsRef<Path> + ?Sized,
    F: FnMut() -> bool + Send + 'static,
{
    let _ = ffmpeg::init();
    ffmpeg::format::input_with_interrupt_and_dictionary(path, interrupt, guarded(extra))
}

/// Whether `e`, from opening `path` here, refuses the file for good: it will never open, however often it is tried.
/// EINVAL is the guard's refusals and other refusals of the file or its name: FFmpeg 9 fails
/// contents that pick a demuxer outside `DEMUXERS` (`libavformat/demux.c`, `avformat_open_input`: "Format not on
/// whitelist") and a name that picks a protocol other than `file` (`libavformat/avio.c`, `ffurl_connect`: "Protocol …
/// not on whitelist") with it, and a demuxer's header checks or a name that wants a number too.
///
/// But FFmpeg passes the OS's own code through, and an OS may say EINVAL for a failure that passes: Windows' C runtime
/// for a network name gone, a device not connected or an online-only cloud file while offline; an NFS or FUSE volume.
/// So EINVAL counts only while the OS reads the file's head now (an open and a 1-byte read, on this error path only);
/// otherwise it may pass, like any other OS error (no such file, no access yet, out of handles, a drive's I/O error).
/// `enable_drefs=0` refuses nothing: a reference movie opens, and its external tracks give no packets
/// (`libavformat/mov.c`).
pub fn refused(e: &ffmpeg::Error, path: &Path) -> bool {
    use std::io::Read;
    *e == ffmpeg::Error::Other {
        errno: ffmpeg::error::EINVAL,
    } && std::fs::File::open(path)
        .and_then(|mut file| file.read(&mut [0u8; 1]))
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whatever `extra` says, external data references stay off, and only the guard's demuxers, protocol and
    /// pattern apply; other options pass.
    #[test]
    fn extra_options_never_loosen_the_guard() {
        let mut extra = ffmpeg::Dictionary::new();
        for (key, value) in [
            ("enable_drefs", "1"),
            ("ignore_loop", "0"),
            ("format_whitelist", "concat,hls"),
            ("protocol_whitelist", "file,http"),
            ("pattern_type", "sequence"),
            ("probesize", "32"),
        ] {
            extra.set(key, value);
        }
        let options = guarded(extra);
        assert_eq!(options.get("enable_drefs"), Some("0"));
        assert_eq!(options.get("ignore_loop"), Some("1"));
        assert_eq!(options.get("format_whitelist"), Some(DEMUXERS));
        assert_eq!(options.get("protocol_whitelist"), Some("file"));
        assert_eq!(options.get("pattern_type"), Some("none"));
        assert_eq!(options.get("probesize"), Some("32"), "other options pass");
    }

    /// Guard 1: a container outside the guard's demuxers (an `ffconcat` list, an MPEG-TS stream) is a refusal; a file
    /// that isn't there is not.
    #[test]
    fn a_refusal_is_told_from_an_os_error() {
        let dir = tempfile::tempdir().unwrap();
        let list = dir.path().join("list.mov");
        std::fs::write(&list, "ffconcat version 1.0\nfile screen.mov\n").unwrap();
        // 64 MPEG-TS null packets: a sync byte every 188 bytes, which FFmpeg's `mpegts` probe takes at its top score.
        let ts = dir.path().join("clip.ts");
        let null = [&[0x47u8, 0x1f, 0xff, 0x10][..], &[0xff; 184][..]].concat();
        std::fs::write(&ts, null.repeat(64)).unwrap();
        for path in [&list, &ts] {
            let e = input(path).err().expect("refused");
            assert!(refused(&e, path), "{}: {e:?}", path.display());
        }
        let gone = dir.path().join("gone.mov");
        let e = input(&gone).err().expect("not there");
        assert!(!refused(&e, &gone), "{e:?}");
        // A protocol other than `file` is refused with the same code; no file of that name reads, so `refused` never
        // counts it (Klipolo's paths are absolute: always `file`).
        let einval = ffmpeg::Error::Other {
            errno: ffmpeg::error::EINVAL,
        };
        assert_eq!(input("data:,hello").err(), Some(einval));
    }
}
