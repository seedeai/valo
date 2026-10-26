//! Runtime effects: WGSL functions the renderer splices into its own pipelines.
//!
//! Every effect defines `fn shade(position: vec2f) -> vec4f`. As a paint shader it colours the
//! shapes it fills; as a filter it may also read `source`, the content it filters or the
//! backdrop behind it. `position` is in the draw's local coordinates, and the colour returned is
//! premultiplied, as images and `source` sample and as Skia's, Impeller's and Metal's shaders
//! return it. The renderer writes the `@fragment` entry around the function, so coverage,
//! clipping, the paint's alpha and blending stay its own and an effect cannot break them.
//!
//! An effect's own bindings follow the renderer's groups. Its images are `texture_2d<f32>` /
//! `sampler` pairs in `@group(1)`, at bindings 0 and 1, 2 and 3, and so on; its uniforms are one
//! struct at `@group(2) @binding(0)`, at most [`MAX_UNIFORM_BYTES`]. A filter's `source` and
//! `source_sampler` are `@group(3)`, read through `uv_of(position)`, the source's uv at a local
//! position, and `source_levels()`, how many levels its pyramid has; `filter_bounds()` is the
//! area the filter shades, `(x, y, width, height)` in local coordinates. `@group(0)` is the
//! renderer's own. Pipeline-overridable
//! constants (`override`) choose variants, compiled one pipeline per set of values; each needs a
//! default, so a draw names only the ones it changes.
//!
//! Creating an effect parses and validates its WGSL with naga, together with the declarations
//! the renderer adds for its kind, and needs no GPU device.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use naga::{AddressSpace, ImageClass, ImageDimension, ScalarKind, TypeInner, VectorSize};

/// The group of an effect's images.
pub const IMAGE_GROUP: u32 = 1;
/// The group of an effect's uniform struct.
pub const UNIFORM_GROUP: u32 = 2;
/// The group of a filter's `source` and `source_sampler`.
pub const SOURCE_GROUP: u32 = 3;
/// The largest uniform struct an effect may declare.
pub const MAX_UNIFORM_BYTES: u32 = 1024;

/// What the renderer declares for every filter. The helpers' bodies here stand in for the
/// renderer's, which read its own draw record; their names and signatures are the contract.
const FILTER_CONTRACT: &str = "
@group(3) @binding(0) var source: texture_2d<f32>;
@group(3) @binding(1) var source_sampler: sampler;
var<private> valo_contract_source: vec4<f32>;

fn uv_of(position: vec2<f32>) -> vec2<f32> {
    return (position - valo_contract_source.xy) / valo_contract_source.zw;
}

fn source_levels() -> f32 {
    return 1.0;
}

fn filter_bounds() -> vec4<f32> {
    return valo_contract_source;
}
";

/// What a filter asks the renderer to prepare as its `source`.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FilterInputs {
    /// How far outside the drawn area the filter reads, in local units: the source covers the
    /// drawn area grown by this much on every side.
    pub margin: f32,
    /// Whether the source carries a blur pyramid: level 0 is the source, each level after it
    /// half the size and blurred, read with `textureSampleLevel`.
    pub source_mips: bool,
    /// The resolution of the source relative to the target; 0.5 captures at half resolution.
    pub source_scale: f32,
}

impl Default for FilterInputs {
    fn default() -> FilterInputs {
        FilterInputs {
            margin: 0.0,
            source_mips: false,
            source_scale: 1.0,
        }
    }
}

/// Whether an effect fills shapes or filters an input.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum EffectKind {
    /// A paint shader, colouring the shapes it fills.
    Shader,
    /// A filter, which may also read `source`.
    Filter(FilterInputs),
}

/// The type of a uniform field, as WGSL lays it out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum UniformType {
    F32,
    I32,
    U32,
    Vec2F,
    Vec3F,
    Vec4F,
    Vec2I,
    Vec2U,
    Mat2x2F,
    Mat3x3F,
    Mat4x4F,
    /// Anything else (arrays, nested structs), written as raw bytes.
    Other {
        size: u32,
    },
}

/// One field of an effect's uniform struct.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct UniformField {
    pub name: String,
    pub offset: u32,
    pub ty: UniformType,
}

/// An effect's uniform struct: the bytes a draw supplies for it.
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct UniformLayout {
    /// The struct's size in bytes; 0 when the effect has no uniforms.
    pub size: u32,
    pub fields: Vec<UniformField>,
}

impl UniformLayout {
    /// The field named `name`.
    pub fn field(&self, name: &str) -> Option<&UniformField> {
        self.fields.iter().find(|field| field.name == name)
    }
}

/// Why WGSL could not become an effect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EffectError {
    /// The WGSL does not parse; the message points into the source.
    Parse(String),
    /// The WGSL parses but naga rejects it.
    Invalid(String),
    /// The WGSL is valid but breaks the effect contract described in this module.
    Contract(String),
}

impl std::fmt::Display for EffectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EffectError::Parse(message) => write!(f, "effect does not parse: {message}"),
            EffectError::Invalid(message) => write!(f, "effect is invalid: {message}"),
            EffectError::Contract(message) => write!(f, "effect breaks the contract: {message}"),
        }
    }
}

impl std::error::Error for EffectError {}

/// A WGSL function the renderer runs as a paint shader or a filter.
///
/// Immutable once created; draws share it through `Arc` and supply uniforms and images per draw
/// in a [`RuntimeShader`].
#[derive(Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct RuntimeEffect {
    /// Identity for the renderer's pipeline caches; unique per created effect.
    id: u64,
    kind: EffectKind,
    source: Arc<str>,
    uniforms: UniformLayout,
    /// The names of the effect's textures, by slot.
    image_names: Vec<String>,
    constants: Vec<String>,
}

impl RuntimeEffect {
    /// A paint shader from WGSL defining `fn shade(position: vec2f) -> vec4f`.
    pub fn shader(wgsl: &str) -> Result<Arc<RuntimeEffect>, EffectError> {
        RuntimeEffect::new(EffectKind::Shader, wgsl)
    }

    /// A filter from WGSL defining `fn shade(position: vec2f) -> vec4f`, which may read
    /// `source`.
    pub fn filter(wgsl: &str, inputs: FilterInputs) -> Result<Arc<RuntimeEffect>, EffectError> {
        // Written so NaN fails too.
        let scale_in_range = inputs.source_scale > 0.0 && inputs.source_scale <= 1.0;
        if !scale_in_range {
            return Err(EffectError::Contract(format!(
                "source_scale must be in (0, 1], not {}",
                inputs.source_scale
            )));
        }
        let margin_in_range = inputs.margin >= 0.0;
        if !margin_in_range {
            return Err(EffectError::Contract(format!(
                "margin must not be negative, not {}",
                inputs.margin
            )));
        }
        RuntimeEffect::new(EffectKind::Filter(inputs), wgsl)
    }

    /// What the renderer declares for every effect of `kind`, for validation.
    fn contract(kind: EffectKind) -> &'static str {
        match kind {
            EffectKind::Shader => "",
            EffectKind::Filter(_) => FILTER_CONTRACT,
        }
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn kind(&self) -> EffectKind {
        self.kind
    }

    /// What a filter asks of its source; `None` for a paint shader.
    pub fn filter_inputs(&self) -> Option<FilterInputs> {
        match self.kind {
            EffectKind::Shader => None,
            EffectKind::Filter(inputs) => Some(inputs),
        }
    }

    /// The effect author's WGSL, without the contract.
    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn uniforms(&self) -> &UniformLayout {
        &self.uniforms
    }

    /// How many images the effect samples.
    pub fn images(&self) -> u32 {
        self.image_names.len() as u32
    }

    /// The names of the effect's textures, by slot: the image at slot `n` is bound at
    /// `@binding(2n)`.
    pub fn image_names(&self) -> &[String] {
        &self.image_names
    }

    /// The names of the effect's `override` constants.
    pub fn constants(&self) -> &[String] {
        &self.constants
    }

    fn new(kind: EffectKind, wgsl: &str) -> Result<Arc<RuntimeEffect>, EffectError> {
        // The author's source comes first so naga's line numbers are theirs.
        let composed = format!("{wgsl}\n{}", RuntimeEffect::contract(kind));
        let module = naga::front::wgsl::parse_str(&composed)
            .map_err(|error| EffectError::Parse(error.emit_to_string(&composed)))?;
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .map_err(|error| EffectError::Invalid(error.emit_to_string(&composed)))?;
        if !module.entry_points.is_empty() {
            return Err(EffectError::Contract(
                "an effect defines no entry points; valo writes the @fragment entry".to_owned(),
            ));
        }
        check_function(&module)?;
        let (uniforms, image_names) = read_bindings(&module, kind)?;
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        Ok(Arc::new(RuntimeEffect {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            kind,
            source: Arc::from(wgsl),
            uniforms,
            image_names,
            constants: read_constants(&module)?,
        }))
    }
}

/// The function every effect defines.
const FUNCTION: &str = "shade";

/// The effect's `shade` function exists with its signature.
fn check_function(module: &naga::Module) -> Result<(), EffectError> {
    let name = FUNCTION;
    let signature = format!("fn {name}(position: vec2<f32>) -> vec4<f32>");
    let Some((_, function)) = module
        .functions
        .iter()
        .find(|(_, function)| function.name.as_deref() == Some(name))
    else {
        return Err(EffectError::Contract(format!("missing `{signature}`")));
    };
    let takes_position = function.arguments.len() == 1
        && is_float_vector(module, function.arguments[0].ty, VectorSize::Bi);
    let returns_colour = function
        .result
        .as_ref()
        .is_some_and(|result| is_float_vector(module, result.ty, VectorSize::Quad));
    if !(takes_position && returns_colour) {
        return Err(EffectError::Contract(format!(
            "`{name}` must be `{signature}`"
        )));
    }
    Ok(())
}

fn is_float_vector(module: &naga::Module, ty: naga::Handle<naga::Type>, size: VectorSize) -> bool {
    matches!(
        module.types[ty].inner,
        TypeInner::Vector { size: vector_size, scalar }
            if vector_size == size && scalar.kind == ScalarKind::Float && scalar.width == 4
    )
}

/// The effect's uniform layout and image count, after checking every binding it declares sits
/// where the contract puts it.
fn read_bindings(
    module: &naga::Module,
    kind: EffectKind,
) -> Result<(UniformLayout, Vec<String>), EffectError> {
    let contract = naga::front::wgsl::parse_str(RuntimeEffect::contract(kind))
        .expect("valo's contract parses");
    let contract_names: Vec<&str> = contract
        .global_variables
        .iter()
        .filter_map(|(_, global)| global.name.as_deref())
        .collect();
    let mut uniforms = UniformLayout::default();
    let mut textures = Vec::new();
    let mut samplers = Vec::new();
    for (_, global) in module.global_variables.iter() {
        let name = global.name.as_deref().unwrap_or("<unnamed>");
        if contract_names.contains(&name) {
            continue;
        }
        let Some(binding) = &global.binding else {
            continue;
        };
        let inner = &module.types[global.ty].inner;
        match (binding.group, binding.binding) {
            (UNIFORM_GROUP, 0) => uniforms = read_uniforms(module, name, inner, global.space)?,
            (IMAGE_GROUP, index) => match inner {
                TypeInner::Image {
                    dim: ImageDimension::D2,
                    arrayed: false,
                    class:
                        ImageClass::Sampled {
                            kind: ScalarKind::Float,
                            multi: false,
                        },
                } if index % 2 == 0 => textures.push((index, name.to_owned())),
                TypeInner::Sampler { comparison: false } if index % 2 == 1 => samplers.push(index),
                _ => {
                    return Err(EffectError::Contract(format!(
                        "`{name}` at @group({IMAGE_GROUP}) @binding({index}): images are \
                         texture_2d<f32> at even bindings, each followed by its sampler"
                    )));
                }
            },
            (group, index) => {
                return Err(EffectError::Contract(format!(
                    "`{name}` at @group({group}) @binding({index}): an effect's images are in \
                     @group({IMAGE_GROUP}) and its uniforms at @group({UNIFORM_GROUP}) @binding(0)"
                )));
            }
        }
    }
    textures.sort_unstable();
    samplers.sort_unstable();
    let paired = samplers.len() == textures.len()
        && textures.iter().enumerate().all(|(slot, (binding, _))| {
            *binding == 2 * slot as u32 && samplers[slot] == binding + 1
        });
    if !paired {
        return Err(EffectError::Contract(format!(
            "images must be texture_2d<f32> / sampler pairs in @group({IMAGE_GROUP}), at bindings \
             0 and 1, 2 and 3, …"
        )));
    }
    Ok((
        uniforms,
        textures.into_iter().map(|(_, name)| name).collect(),
    ))
}

fn read_uniforms(
    module: &naga::Module,
    name: &str,
    inner: &TypeInner,
    space: AddressSpace,
) -> Result<UniformLayout, EffectError> {
    let (TypeInner::Struct { members, span }, AddressSpace::Uniform) = (inner, space) else {
        return Err(EffectError::Contract(format!(
            "@group({UNIFORM_GROUP}) @binding(0) is the effect's uniform struct; `{name}` is not one"
        )));
    };
    if *span > MAX_UNIFORM_BYTES {
        return Err(EffectError::Contract(format!(
            "the uniform struct is {span} bytes; an effect's uniforms fit in {MAX_UNIFORM_BYTES}"
        )));
    }
    Ok(UniformLayout {
        size: *span,
        fields: members
            .iter()
            .map(|member| UniformField {
                name: member.name.clone().unwrap_or_default(),
                offset: member.offset,
                ty: uniform_type(module, member.ty),
            })
            .collect(),
    })
}

fn uniform_type(module: &naga::Module, ty: naga::Handle<naga::Type>) -> UniformType {
    let inner = &module.types[ty].inner;
    match *inner {
        TypeInner::Scalar(scalar) if scalar.width == 4 => match scalar.kind {
            ScalarKind::Float => UniformType::F32,
            ScalarKind::Sint => UniformType::I32,
            ScalarKind::Uint => UniformType::U32,
            _ => UniformType::Other {
                size: inner.size(module.to_ctx()),
            },
        },
        TypeInner::Vector { size, scalar } if scalar.width == 4 => match (size, scalar.kind) {
            (VectorSize::Bi, ScalarKind::Float) => UniformType::Vec2F,
            (VectorSize::Tri, ScalarKind::Float) => UniformType::Vec3F,
            (VectorSize::Quad, ScalarKind::Float) => UniformType::Vec4F,
            (VectorSize::Bi, ScalarKind::Sint) => UniformType::Vec2I,
            (VectorSize::Bi, ScalarKind::Uint) => UniformType::Vec2U,
            _ => UniformType::Other {
                size: inner.size(module.to_ctx()),
            },
        },
        TypeInner::Matrix {
            columns,
            rows,
            scalar,
        } if scalar.kind == ScalarKind::Float && scalar.width == 4 && columns == rows => {
            match columns {
                VectorSize::Bi => UniformType::Mat2x2F,
                VectorSize::Tri => UniformType::Mat3x3F,
                VectorSize::Quad => UniformType::Mat4x4F,
            }
        }
        _ => UniformType::Other {
            size: inner.size(module.to_ctx()),
        },
    }
}

/// The names of the effect's `override` constants, each keyed by its name and given a default.
fn read_constants(module: &naga::Module) -> Result<Vec<String>, EffectError> {
    module
        .overrides
        .iter()
        .map(|(_, constant)| {
            let name = constant.name.clone().unwrap_or_default();
            if constant.id.is_some() {
                return Err(EffectError::Contract(format!(
                    "override `{name}` has an @id; valo sets overrides by name"
                )));
            }
            if constant.init.is_none() {
                return Err(EffectError::Contract(format!(
                    "override `{name}` needs a default"
                )));
            }
            Ok(name)
        })
        .collect()
}

/// An image an effect samples, and how.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct EffectImage {
    pub image: crate::Image,
    pub sampling: crate::Sampling,
}

/// A [`RuntimeEffect`] with one draw's uniforms and images.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct RuntimeShader {
    pub effect: Arc<RuntimeEffect>,
    /// The uniform struct's bytes, exactly [`UniformLayout::size`] long.
    pub uniforms: Arc<[u8]>,
    /// One image, with how it is sampled, per image pair the effect declares.
    pub images: Vec<EffectImage>,
    /// Values for the effect's `override` constants, by name; unset ones keep their default.
    pub constants: Vec<(String, f64)>,
    /// How far past its area this draw's filter reads, in place of the effect's
    /// [`FilterInputs::margin`]: for a filter whose reach follows its uniforms.
    pub margin: Option<f32>,
}

impl RuntimeShader {
    /// Pairs `effect` with a draw's inputs, checking they fit what it declares.
    pub fn new(
        effect: Arc<RuntimeEffect>,
        uniforms: impl Into<Arc<[u8]>>,
        images: Vec<EffectImage>,
    ) -> Result<RuntimeShader, EffectError> {
        let uniforms = uniforms.into();
        if uniforms.len() != effect.uniforms.size as usize {
            return Err(EffectError::Contract(format!(
                "the effect's uniforms are {} bytes, not {}",
                effect.uniforms.size,
                uniforms.len()
            )));
        }
        if images.len() != effect.image_names.len() {
            return Err(EffectError::Contract(format!(
                "the effect samples {} images, not {}",
                effect.image_names.len(),
                images.len()
            )));
        }
        Ok(RuntimeShader {
            effect,
            uniforms,
            images,
            constants: Vec::new(),
            margin: None,
        })
    }

    /// Sets how far past its area this draw's filter reads, in local units, in place of the
    /// effect's [`FilterInputs::margin`].
    pub fn margin(mut self, margin: f32) -> Result<RuntimeShader, EffectError> {
        if self.effect.filter_inputs().is_none() {
            return Err(EffectError::Contract(
                "only a filter reads past its area".to_owned(),
            ));
        }
        // Written so NaN fails too.
        let margin_in_range = margin >= 0.0;
        if !margin_in_range {
            return Err(EffectError::Contract(format!(
                "margin must not be negative, not {margin}"
            )));
        }
        self.margin = Some(margin);
        Ok(self)
    }

    /// How far past its area this draw's filter reads: its own margin, or its effect's.
    pub fn read_margin(&self) -> f32 {
        self.margin.unwrap_or_else(|| {
            self.effect
                .filter_inputs()
                .map_or(0.0, |inputs| inputs.margin)
        })
    }

    /// Sets the `override` constant `name` for this draw, choosing a pipeline variant.
    pub fn constant(mut self, name: &str, value: f64) -> Result<RuntimeShader, EffectError> {
        if !self
            .effect
            .constants
            .iter()
            .any(|constant| constant == name)
        {
            return Err(EffectError::Contract(format!(
                "the effect declares no override `{name}`"
            )));
        }
        self.constants.retain(|(existing, _)| existing != name);
        self.constants.push((name.to_owned(), value));
        Ok(self)
    }
}

impl PartialEq for RuntimeShader {
    fn eq(&self, other: &RuntimeShader) -> bool {
        Arc::ptr_eq(&self.effect, &other.effect)
            && self.uniforms == other.uniforms
            && self.images == other.images
            && self.constants == other.constants
            && self.margin == other.margin
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WAVE: &str = "
struct Uniforms {
    time: f32,
    tint: vec4<f32>,
    centre: vec3<f32>,
}
@group(2) @binding(0) var<uniform> u: Uniforms;

fn shade(position: vec2<f32>) -> vec4<f32> {
    return u.tint * (0.5 + 0.5 * sin(position.x * 0.1 + u.time));
}
";

    #[test]
    fn a_shader_reads_its_uniform_layout() {
        let effect = RuntimeEffect::shader(WAVE).expect("valid shader");
        let uniforms = effect.uniforms();
        assert_eq!(uniforms.size, 48);
        let fields: Vec<_> = uniforms
            .fields
            .iter()
            .map(|field| (field.name.as_str(), field.offset, field.ty))
            .collect();
        assert_eq!(
            fields,
            vec![
                ("time", 0, UniformType::F32),
                ("tint", 16, UniformType::Vec4F),
                ("centre", 32, UniformType::Vec3F),
            ]
        );
        assert_eq!(effect.images(), 0);
    }

    #[test]
    fn a_filter_may_use_the_contract() {
        let effect = RuntimeEffect::filter(
            "
@group(1) @binding(0) var noise: texture_2d<f32>;
@group(1) @binding(1) var noise_sampler: sampler;

fn shade(position: vec2<f32>) -> vec4<f32> {
    let lod = source_levels() - 1.0;
    let grain = textureSampleLevel(noise, noise_sampler, position / 64.0, 0.0);
    return textureSampleLevel(source, source_sampler, uv_of(position), lod) * grain;
}
",
            FilterInputs {
                margin: 12.0,
                source_mips: true,
                source_scale: 0.5,
            },
        )
        .expect("valid filter");
        assert_eq!(effect.images(), 1);
        assert_eq!(effect.image_names(), ["noise"]);
        let shader = RuntimeShader::new(effect.clone(), Vec::new(), vec![]);
        assert!(shader.is_err(), "the noise image is missing");
        assert_eq!(effect.uniforms().size, 0);
    }

    #[test]
    fn override_constants_choose_variants() {
        let effect = RuntimeEffect::shader(
            "
override highlights: bool = true;
override quality: f32 = 1.0;
fn shade(position: vec2<f32>) -> vec4<f32> {
    if highlights { return vec4<f32>(quality); }
    return vec4<f32>(0.0);
}
",
        )
        .expect("valid shader");
        assert_eq!(effect.constants(), ["highlights", "quality"]);
        let shader = RuntimeShader::new(effect, Vec::new(), Vec::new())
            .unwrap()
            .constant("quality", 2.0)
            .unwrap();
        assert_eq!(shader.constants, vec![("quality".to_owned(), 2.0)]);
        assert!(shader.constant("missing", 1.0).is_err());
        let without_default = RuntimeEffect::shader(
            "override quality: f32;
             fn shade(position: vec2<f32>) -> vec4<f32> { return vec4<f32>(quality); }",
        );
        assert!(contract_error(without_default).contains("needs a default"));
    }

    /// A draw may read further than its effect says, as a filter whose reach follows its
    /// uniforms does; a paint shader reads nothing past its area.
    #[test]
    fn a_draw_may_set_its_own_margin() {
        let filter = RuntimeEffect::filter(
            "fn shade(position: vec2<f32>) -> vec4<f32> {
                 return textureSampleLevel(source, source_sampler, uv_of(position), 0.0);
             }",
            FilterInputs {
                margin: 4.0,
                ..FilterInputs::default()
            },
        )
        .unwrap();
        let shader = RuntimeShader::new(filter, Vec::new(), Vec::new()).unwrap();
        assert_eq!(shader.read_margin(), 4.0);
        let wider = shader.clone().margin(20.0).unwrap();
        assert_eq!(wider.read_margin(), 20.0);
        assert_ne!(wider, shader);
        assert!(shader.margin(f32::NAN).is_err());
        let paint = RuntimeEffect::shader(
            "fn shade(position: vec2<f32>) -> vec4<f32> { return vec4<f32>(1.0); }",
        )
        .unwrap();
        let paint = RuntimeShader::new(paint, Vec::new(), Vec::new()).unwrap();
        assert!(paint.margin(1.0).is_err());
    }

    fn contract_error(result: Result<Arc<RuntimeEffect>, EffectError>) -> String {
        match result {
            Err(EffectError::Contract(message)) => message,
            other => panic!("expected a contract error, got {other:?}"),
        }
    }

    #[test]
    fn the_function_must_exist_with_its_signature() {
        let missing = RuntimeEffect::shader(
            "fn paint(position: vec2<f32>) -> vec4<f32> { return vec4<f32>(1.0); }",
        );
        assert!(contract_error(missing).contains("missing `fn shade"));
        let wrong = RuntimeEffect::shader(
            "fn shade(position: vec2<f32>) -> vec3<f32> { return vec3<f32>(1.0); }",
        );
        assert!(contract_error(wrong).contains("must be"));
        let shader_reading_source = RuntimeEffect::shader(
            "fn shade(position: vec2<f32>) -> vec4<f32> { return textureLoad(source, vec2<i32>(0), 0); }",
        );
        assert!(
            matches!(shader_reading_source, Err(EffectError::Parse(_))),
            "only filters have a source"
        );
    }

    #[test]
    fn bindings_must_sit_where_the_contract_puts_them() {
        let renderer_group = RuntimeEffect::shader(
            "@group(0) @binding(5) var<uniform> x: vec4<f32>;
             fn shade(position: vec2<f32>) -> vec4<f32> { return x; }",
        );
        assert!(contract_error(renderer_group).contains("@group(0)"));
        let lone_texture = RuntimeEffect::shader(
            "@group(1) @binding(0) var t: texture_2d<f32>;
             fn shade(position: vec2<f32>) -> vec4<f32> { return textureLoad(t, vec2<i32>(0), 0); }",
        );
        assert!(contract_error(lone_texture).contains("pairs"));
        let entry_point = RuntimeEffect::shader(
            "fn shade(position: vec2<f32>) -> vec4<f32> { return vec4<f32>(1.0); }
             @fragment fn main() -> @location(0) vec4<f32> { return shade(vec2<f32>(0.0)); }",
        );
        assert!(contract_error(entry_point).contains("entry points"));
    }

    #[test]
    fn uniforms_have_a_size_limit() {
        let oversized = RuntimeEffect::shader(
            "struct Big { values: array<vec4<f32>, 65> }
             @group(2) @binding(0) var<uniform> big: Big;
             fn shade(position: vec2<f32>) -> vec4<f32> { return big.values[0]; }",
        );
        assert!(contract_error(oversized).contains("1024"));
    }

    #[test]
    fn a_parse_error_points_into_the_authors_source() {
        let Err(EffectError::Parse(message)) =
            RuntimeEffect::shader("fn shade(position: vec2<f32>) -> vec4<f32> { return oops; }")
        else {
            panic!("expected a parse error");
        };
        assert!(message.contains("oops"), "{message}");
        assert!(message.contains(":1:"), "{message}");
    }

    #[test]
    fn draw_inputs_must_fit_the_effect() {
        let effect = RuntimeEffect::shader(WAVE).unwrap();
        assert!(RuntimeShader::new(effect.clone(), vec![0; 48], Vec::new()).is_ok());
        assert!(RuntimeShader::new(effect, vec![0; 16], Vec::new()).is_err());
        assert!(RuntimeEffect::filter(
            WAVE,
            FilterInputs {
                source_scale: 0.0,
                ..FilterInputs::default()
            }
        )
        .is_err());
    }
}
