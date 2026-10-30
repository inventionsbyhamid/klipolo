//! An instance's text on the GPU: its atlas's textures,
//! made and filled where the atlas was drawn, on the text build thread (`upload_atlas`, so the render thread no
//! longer pays a 16 MiB atlas's copy itself; it can still wait for it, as wgpu-core holds the queue's
//! pending-writes lock across `write_texture`'s staging copy and a frame's `submit` and `write_buffer` take that lock,
//! and a validation error raised by an upload there faults whichever frame the render thread is on), and its draws and
//! paints (a piece's own values, 112 bytes, and what a node's pieces of one look share, once a frame),
//! with their bind group. A frame writes only the range of draws and of paints that changed.

use bytemuck::{Pod, Zeroable};

use super::text_atlas::AtlasImage;
use super::text_blur_pass::TextBlur;
use super::text_pipeline::{Bound, TextPipeline};
use crate::render::effects::made;

/// One piece or background (`TextDraw` in `text.wgsl`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(super) struct TextDraw {
    /// Box pixels → target pixels: the 2 × 2 part (a, b, c, d), then the translation, the kind (0 a piece, 1 a span's
    /// background) and a background's corner radius in box pixels.
    pub m: [f32; 4],
    pub t: [f32; 4],
    /// The quad in box pixels where it rests (left, top, right, bottom), and its cell in the atlas (0–1).
    pub rect: [f32; 4],
    pub uv: [f32; 4],
    /// Its opacity, its paint (its index in the frame's paints), how far its quad grows for its blur (its Gaussian in
    /// box pixels; 0 sharp), and that Gaussian in atlas texels (0: it reads no blur back).
    pub misc: [f32; 4],
    /// A blurring piece's blur in the scratch target (`text_blur.rs`): its pixel is a.xy + atlas uv × a.zw, within b.
    pub blur_a: [f32; 4],
    pub blur_b: [f32; 4],
}

/// What a node's pieces of one look draw with this frame (`TextPaint` in `text.wgsl`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(super) struct TextPaintDraw {
    /// Straight RGBA: the fill (a gradient's tint), and the outline.
    pub fill: [f32; 4],
    pub outline: [f32; 4],
    /// The gradient (−1 none), its turn (radians) and slide, and 1 when its colour glyphs draw.
    pub paint: [f32; 4],
    /// The gradient's box: its size and its top left, in box pixels.
    pub gbox: [f32; 4],
    /// Three clips (`clip_distance`): the box a fitted text keeps to, the node's own, its ancestor's.
    pub clips: [[f32; 4]; 9],
    /// Its baked shadow: its colour (straight; alpha 0, none), its offset in box pixels, and its cell's box (box
    /// pixels, where it rests) and uv in the shadows' texture.
    pub shadow: [f32; 4],
    pub shadow_at: [f32; 4],
    pub shadow_rect: [f32; 4],
    pub shadow_uv: [f32; 4],
    /// Where the colour texture lies in the atlas: its top left in atlas texels, and one over its size.
    pub cmap: [f32; 4],
}

/// An atlas's textures, made and filled on the text build thread: its masks, its colour glyphs' (over only the texels
/// they cover: `TextLayouts::cmap`), its baked shadows, and the bytes they hold.
pub(super) struct AtlasTextures {
    masks: wgpu::Texture,
    colors: Option<wgpu::Texture>,
    shadows: Option<wgpu::Texture>,
    pub bytes: u64,
}

/// `image`'s textures, uploaded now (the build thread: the queue orders them before the render thread's next frame).
pub(super) fn upload_atlas(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    image: &AtlasImage,
) -> AtlasTextures {
    use super::text_blit::atlas_texture;
    use wgpu::TextureFormat::{R8Unorm, Rg8Unorm, Rgba8Unorm};
    let masks = atlas_texture(device, queue, image.size, Rg8Unorm, &image.masks);
    let colors = image
        .colors
        .as_ref()
        .map(|c| atlas_texture(device, queue, c.size, Rgba8Unorm, &c.pixels));
    let shadows = image
        .shadows
        .as_ref()
        .map(|s| atlas_texture(device, queue, s.size, R8Unorm, &s.pixels));
    AtlasTextures {
        masks,
        colors,
        shadows,
        bytes: image.bytes(),
    }
}

/// An instance's text on the GPU: its atlas's textures and views, its draws and paints (room for every piece and
/// background a frame can show), their bind group, and what the buffers hold now.
pub(super) struct TextGpu {
    _atlas: AtlasTextures,
    pub(super) masks_view: wgpu::TextureView,
    colors_view: Option<wgpu::TextureView>,
    shadows_view: Option<wgpu::TextureView>,
    buffer: wgpu::Buffer,
    paints: wgpu::Buffer,
    capacity: usize,
    paint_room: usize,
    group: wgpu::BindGroup,
    /// Its blurring pieces' passes (made the first frame one blurs), and whether this frame's draws read them.
    pub(super) blur: Option<TextBlur>,
    pub(super) use_blur: bool,
    uploaded: Vec<TextDraw>,
    uploaded_paints: Vec<TextPaintDraw>,
    pub(super) bytes: u64,
}

impl TextGpu {
    /// An instance's text on the GPU: `atlas` (uploaded on the build thread) and room for `draws` pieces and
    /// backgrounds and the `paints` they name.
    pub(super) fn new(
        device: &wgpu::Device,
        tp: &TextPipeline,
        atlas: AtlasTextures,
        (draws, paints): (usize, usize),
    ) -> Self {
        let (capacity, paint_room) = (draws.max(1), paints.max(1));
        let buffer = |label, bytes: usize| {
            made();
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: bytes as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let draw = capacity * std::mem::size_of::<TextDraw>();
        let paint = paint_room * std::mem::size_of::<TextPaintDraw>();
        let (draws_buffer, paints) = (
            buffer("Klipolo scene text draws", draw),
            buffer("Klipolo scene text paints", paint),
        );
        let view = |t: &wgpu::Texture| t.create_view(&wgpu::TextureViewDescriptor::default());
        let masks_view = view(&atlas.masks);
        let colors_view = atlas.colors.as_ref().map(view);
        let shadows_view = atlas.shadows.as_ref().map(view);
        let bytes = atlas.bytes + 2 * (draw + paint) as u64;
        let group = tp.group(
            device,
            Bound {
                draws: &draws_buffer,
                paints: &paints,
                masks: &masks_view,
                colors: colors_view.as_ref(),
                shadows: shadows_view.as_ref(),
                blurred: None,
            },
        );
        Self {
            _atlas: atlas,
            masks_view,
            colors_view,
            shadows_view,
            buffer: draws_buffer,
            paints,
            capacity,
            paint_room,
            group,
            blur: None,
            use_blur: false,
            uploaded: Vec::with_capacity(capacity),
            uploaded_paints: Vec::with_capacity(paint_room),
            bytes,
        }
    }

    /// Its bind group, with the frame's blur read back from `blurred` (None: a transparent texel).
    pub(super) fn bind(
        &self,
        tp: &TextPipeline,
        device: &wgpu::Device,
        blurred: Option<&wgpu::TextureView>,
    ) -> wgpu::BindGroup {
        tp.group(
            device,
            Bound {
                draws: &self.buffer,
                paints: &self.paints,
                masks: &self.masks_view,
                colors: self.colors_view.as_ref(),
                shadows: self.shadows_view.as_ref(),
                blurred,
            },
        )
    }

    /// The bind group this frame's draws read: with its blurring pieces' scratch target when they blur.
    pub(super) fn group(&self) -> &wgpu::BindGroup {
        match (&self.blur, self.use_blur) {
            (Some(TextBlur { with: Some(b), .. }), true) => &b.text,
            _ => &self.group,
        }
    }

    /// Writes the range of `draws` and of `paints` (at most the capacity) the buffers don't hold yet: no allocation once
    /// warm. How many draws there are, and the bytes written.
    pub(super) fn upload(
        &mut self,
        queue: &wgpu::Queue,
        draws: &[TextDraw],
        paints: &[TextPaintDraw],
    ) -> (usize, u64) {
        let n = draws.len().min(self.capacity);
        let written = write_changed(queue, &self.buffer, &mut self.uploaded, &draws[..n])
            + write_changed(
                queue,
                &self.paints,
                &mut self.uploaded_paints,
                &paints[..paints.len().min(self.paint_room)],
            );
        (n, written)
    }
}

/// `now` written into `buffer` where it differs from `kept` (what the buffer holds), as one range: the bytes written.
fn write_changed<T: Pod + PartialEq>(
    queue: &wgpu::Queue,
    buffer: &wgpu::Buffer,
    kept: &mut Vec<T>,
    now: &[T],
) -> u64 {
    let Some((lo, hi)) = changed(kept, now) else {
        kept.truncate(now.len());
        return 0;
    };
    let at = (lo * std::mem::size_of::<T>()) as u64;
    queue.write_buffer(buffer, at, bytemuck::cast_slice(&now[lo..hi]));
    kept.resize(now.len(), T::zeroed());
    kept[lo..hi].copy_from_slice(&now[lo..hi]);
    ((hi - lo) * std::mem::size_of::<T>()) as u64
}

/// The one range `[lo, hi)` of `now` that differs from `kept` (all past `kept`'s end counts as changed), or None.
fn changed<T: PartialEq>(kept: &[T], now: &[T]) -> Option<(usize, usize)> {
    let same = kept.len().min(now.len());
    let lo = (0..same)
        .find(|&i| kept[i] != now[i])
        .or((same < now.len()).then_some(same))?;
    let hi = if same < now.len() {
        now.len()
    } else {
        (lo..same)
            .rev()
            .find(|&i| kept[i] != now[i])
            .map_or(lo + 1, |h| h + 1)
    };
    Some((lo, hi))
}

#[cfg(test)]
mod tests {
    use super::changed;

    /// A frame writes the one range of draws that changed (a letter rising in a long title: a few draws), all of
    /// them past what the buffer held, and nothing when nothing changed.
    #[test]
    fn a_frame_writes_only_the_range_that_changed() {
        let kept = [1, 2, 3, 4, 5, 6];
        assert_eq!(changed(&kept, &[1, 2, 3, 4, 5, 6]), None);
        assert_eq!(changed(&kept, &[1, 2, 9, 4, 8, 6]), Some((2, 5)));
        assert_eq!(changed(&kept, &[1, 2, 3, 4, 5, 6, 7]), Some((6, 7)));
        assert_eq!(changed(&kept, &[1, 2, 0, 4, 5, 6, 7]), Some((2, 7)));
        assert_eq!(changed(&kept, &[1, 2, 3]), None, "fewer: nothing to write");
        assert_eq!(changed(&[] as &[i32], &[4]), Some((0, 1)));
    }
}
