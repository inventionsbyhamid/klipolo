//! The shapes pipeline's inter-stage budget (a standing lane-B note): its fragment stage reads at most 56
//! of the 60 varying components a Metal or Vulkan device guarantees, so the fills and backdrops ride in fields it
//! already had.

use super::SOURCE;

/// The components of `ty` (a scalar or a vector of `naga`'s module).
fn components(module: &naga::Module, ty: naga::Handle<naga::Type>) -> u32 {
    match module.types[ty].inner {
        naga::TypeInner::Scalar(_) => 1,
        naga::TypeInner::Vector { size, .. } => size as u32,
        _ => 4,
    }
}

#[test]
fn the_shapes_fragment_stage_reads_at_most_56_varyings() {
    let module = naga::front::wgsl::parse_str(SOURCE).expect("the shapes shader parses");
    let fs = module
        .entry_points
        .iter()
        .find(|e| e.name == "fs_main")
        .expect("fs_main");
    let mut read = 0;
    for arg in &fs.function.arguments {
        if let naga::TypeInner::Struct { members, .. } = &module.types[arg.ty].inner {
            for m in members {
                if matches!(m.binding, Some(naga::Binding::Location { .. })) {
                    read += components(&module, m.ty);
                }
            }
        }
    }
    assert!(
        read <= 56,
        "the shapes pipeline reads {read} varying components"
    );
}
