//! The module a device compiles for WGSL that isn't built in. The host adds one loop
//! counter per pixel, `kl_trips`, and opens every loop's body, at any depth, with
//! `if (kl_trips >= 4096u) { break; } kl_trips = kl_trips + 1u;`: all loops together run at most [`LOOP_TRIPS`]
//! times per pixel, whatever their nesting. naga writes the module back out as WGSL, and that text, which is what
//! wgpu compiles, is parsed, validated and walked again: it must keep the host's entry points, open every loop
//! with the check, return through the host's output check, and make the same calls as the module it was written from
//! (naga's writer renames functions, and a name that became a built-in's would take that built-in's calls). Anything
//! else refuses the shader (`harden.roundtrip`): it fails closed. [`verify`] is the same check on text alone, which
//! the effect stage's compile runs on every shader that isn't built in (`ShaderEffect::compilable`).

use naga::valid::{Capabilities, ValidationFlags, Validator};
use naga::{
    AddressSpace, BinaryOperator, Block, Expression, Function, GlobalVariable, Handle, Literal,
    Module, Scalar, ShaderStage, Span, Statement, Type, TypeInner,
};

use super::{Checked, Diagnostic, LOOP_TRIPS};

mod located;
mod output;
mod walk;

pub use walk::{LoopGuards, loop_guards};

/// The host's loop counter.
pub const TRIPS: &str = "kl_trips";
/// The host's entry points, with their stages, by name.
const ENTRY_POINTS: [(&str, ShaderStage); 2] = [
    ("kl_fs", ShaderStage::Fragment),
    ("kl_vs", ShaderStage::Vertex),
];

/// `checked`'s module with the loop counter, written by naga, then read back and checked. The text is the only
/// form of the shader any device compiles.
pub fn harden(checked: &Checked) -> Result<String, Vec<Diagnostic>> {
    let mut module = checked.module.clone();
    let u32_ty = module.types.insert(
        Type {
            name: None,
            inner: TypeInner::Scalar(Scalar::U32),
        },
        Span::UNDEFINED,
    );
    let trips = module.global_variables.append(
        GlobalVariable {
            name: Some(TRIPS.into()),
            space: AddressSpace::Private,
            binding: None,
            ty: u32_ty,
            init: None,
        },
        Span::UNDEFINED,
    );
    for (_, f) in module.functions.iter_mut() {
        let body = std::mem::take(&mut f.body);
        f.body = guard_block(f, body, trips);
    }
    for ep in &mut module.entry_points {
        let body = std::mem::take(&mut ep.function.body);
        ep.function.body = guard_block(&mut ep.function, body, trips);
    }
    let info = validator()
        .validate(&module)
        .map_err(|e| vec![located::validation_failed(checked, &e)])?;
    let text =
        naga::back::wgsl::write_string(&module, &info, naga::back::wgsl::WriterFlags::empty())
            .map_err(|e| {
                vec![Diagnostic::error(
                    "harden.emit",
                    format!("The host couldn't write the shader back out: {e}"),
                )]
            })?;
    roundtrip(&text, &module).map_err(|why| {
        vec![Diagnostic::error(
            "harden.roundtrip",
            format!("The host's re-emitted shader didn't hold, so it's refused: {why}"),
        )]
    })?;
    Ok(text)
}

/// What wgpu will compile is `text`, so it must hold as [`verify`] asks, and make the same calls, to the same functions
/// in the same order, as `hardened`, the module it was written from.
fn roundtrip(text: &str, hardened: &Module) -> Result<(), String> {
    let again = verify(text)?;
    let (before, after) = (walk::calls(hardened), walk::calls(&again));
    if before != after {
        let count = |calls: &[Vec<usize>]| calls.iter().map(Vec::len).sum::<usize>();
        let (n, m) = (count(&before), count(&after));
        return Err(if m > n {
            format!(
                "{} of WGSL's built-in calls became calls to one of its functions, whose name became the built-in's",
                m - n
            )
        } else {
            format!("its calls changed ({n} before, {m} after)")
        });
    }
    Ok(())
}

/// Whether `text` is a hardened module: it parses and validates, keeps the host's entry points, opens every loop with
/// the counter check, and returns through the host's output check. The module, when it is.
pub fn verify(text: &str) -> Result<Module, String> {
    let again = naga::front::wgsl::parse_str(text).map_err(|e| e.emit_to_string(text))?;
    validator()
        .validate(&again)
        .map_err(|e| e.emit_to_string(text))?;
    let mut entries: Vec<_> = again
        .entry_points
        .iter()
        .map(|e| (e.name.as_str(), e.stage))
        .collect();
    entries.sort_by_key(|(name, _)| *name);
    if entries[..] != ENTRY_POINTS[..] {
        return Err(format!("its entry points became {entries:?}"));
    }
    let g = loop_guards(&again);
    if g.guarded != g.loops {
        return Err(format!(
            "{} of its {} loops lost their counter check",
            g.loops - g.guarded,
            g.loops
        ));
    }
    output::intact(&again)?;
    Ok(again)
}

fn validator() -> Validator {
    Validator::new(ValidationFlags::all(), Capabilities::empty())
}

/// `block` with every loop's body (at any depth) opening with the shared check:
/// `if (kl_trips >= 4096u) { break; } kl_trips = kl_trips + 1u;`
fn guard_block(f: &mut Function, block: Block, trips: Handle<GlobalVariable>) -> Block {
    let mut out = Block::with_capacity(block.len());
    for (stmt, span) in block.span_into_iter() {
        let stmt = match stmt {
            Statement::Loop {
                body,
                continuing,
                break_if,
            } => {
                let inner = guard_block(f, body, trips);
                let mut guarded = check_prefix(f, trips);
                guarded.extend_block(inner);
                Statement::Loop {
                    body: guarded,
                    continuing: guard_block(f, continuing, trips),
                    break_if,
                }
            }
            Statement::Block(b) => Statement::Block(guard_block(f, b, trips)),
            Statement::If {
                condition,
                accept,
                reject,
            } => Statement::If {
                condition,
                accept: guard_block(f, accept, trips),
                reject: guard_block(f, reject, trips),
            },
            Statement::Switch { selector, cases } => Statement::Switch {
                selector,
                cases: cases
                    .into_iter()
                    .map(|c| naga::SwitchCase {
                        body: guard_block(f, c.body, trips),
                        ..c
                    })
                    .collect(),
            },
            other => other,
        };
        out.push(stmt, span);
    }
    out
}

fn check_prefix(f: &mut Function, trips: Handle<GlobalVariable>) -> Block {
    let e = &mut f.expressions;
    // Pointers and literals are never emitted; the load and the two binaries are, in one range.
    let ptr = e.append(Expression::GlobalVariable(trips), Span::UNDEFINED);
    let limit = e.append(
        Expression::Literal(Literal::U32(LOOP_TRIPS)),
        Span::UNDEFINED,
    );
    let one = e.append(Expression::Literal(Literal::U32(1)), Span::UNDEFINED);
    let cur = e.append(Expression::Load { pointer: ptr }, Span::UNDEFINED);
    let over = e.append(
        Expression::Binary {
            op: BinaryOperator::GreaterEqual,
            left: cur,
            right: limit,
        },
        Span::UNDEFINED,
    );
    let next = e.append(
        Expression::Binary {
            op: BinaryOperator::Add,
            left: cur,
            right: one,
        },
        Span::UNDEFINED,
    );
    let mut b = Block::new();
    b.push(
        Statement::Emit(naga::Range::new_from_bounds(cur, next)),
        Span::UNDEFINED,
    );
    let mut brk = Block::new();
    brk.push(Statement::Break, Span::UNDEFINED);
    b.push(
        Statement::If {
            condition: over,
            accept: brk,
            reject: Block::new(),
        },
        Span::UNDEFINED,
    );
    b.push(
        Statement::Store {
            pointer: ptr,
            value: next,
        },
        Span::UNDEFINED,
    );
    b
}
