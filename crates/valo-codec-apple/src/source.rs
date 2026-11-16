//! An open `CGImageSource` and what its header says: its size, and whether its images are the
//! frames of an animation or pictures of their own, as a TIFF's pages, an icon's sizes or a HEIF
//! collection's images are, of which one is shown.
use core_foundation::base::{CFType, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::data::{CFData, CFDataRef};
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::image::CGImage;
use core_graphics::sys::CGImageRef;
use foreign_types::ForeignType;
use std::ffi::c_void;
use std::ptr;
use std::time::Duration;
use valo_codec::{DecodeError, OpenError, Repetition};

/// An opaque ImageIO source; only pointers to it ever cross this boundary.
#[repr(C)]
struct CGImageSource(c_void);
type CGImageSourceRef = *mut CGImageSource;

#[link(name = "ImageIO", kind = "framework")]
extern "C" {
    fn CGImageSourceCreateWithData(data: CFDataRef, options: CFDictionaryRef) -> CGImageSourceRef;
    /// The container's type, or null when ImageIO does not recognise the bytes at all.
    fn CGImageSourceGetType(source: CGImageSourceRef) -> CFStringRef;
    fn CGImageSourceGetCount(source: CGImageSourceRef) -> usize;
    fn CGImageSourceCopyProperties(
        source: CGImageSourceRef,
        options: CFDictionaryRef,
    ) -> CFDictionaryRef;
    fn CGImageSourceCopyPropertiesAtIndex(
        source: CGImageSourceRef,
        index: usize,
        options: CFDictionaryRef,
    ) -> CFDictionaryRef;
    fn CGImageSourceCreateThumbnailAtIndex(
        source: CGImageSourceRef,
        index: usize,
        options: CFDictionaryRef,
    ) -> CGImageRef;
    static kCGImageSourceCreateThumbnailFromImageAlways: CFStringRef;
    static kCGImageSourceCreateThumbnailWithTransform: CFStringRef;
    static kCGImageSourceThumbnailMaxPixelSize: CFStringRef;
    static kCGImagePropertyPixelWidth: CFStringRef;
    static kCGImagePropertyPixelHeight: CFStringRef;
    static kCGImagePropertyOrientation: CFStringRef;
    static kCGImagePropertyGIFDictionary: CFStringRef;
    static kCGImagePropertyGIFLoopCount: CFStringRef;
    static kCGImagePropertyGIFDelayTime: CFStringRef;
    static kCGImagePropertyGIFUnclampedDelayTime: CFStringRef;
    static kCGImagePropertyPNGDictionary: CFStringRef;
    static kCGImagePropertyAPNGLoopCount: CFStringRef;
    static kCGImagePropertyAPNGDelayTime: CFStringRef;
    static kCGImagePropertyAPNGUnclampedDelayTime: CFStringRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(value: *const c_void);
}

type Properties = CFDictionary<CFString, CFType>;

/// Source is an open ImageIO source whose header has been read.
pub(crate) struct Source {
    raw: CGImageSourceRef,
    pictures: Pictures,
    oriented_size: [u32; 2],
}

/// How a container's images are shown.
#[derive(Clone, Copy, Debug)]
enum Pictures {
    /// The frames of an animation, played as its loop count says.
    Animation {
        frame_count: u32,
        repetition: Repetition,
    },
    /// One image, the container's `index`th: its only one, or, of several that do not animate,
    /// the one it names as its primary.
    Still { index: usize },
}

impl Source {
    /// `open` recognises the container and reads its size; unknown bytes are a decline.
    pub(crate) fn open(encoded: &[u8]) -> Result<Self, OpenError> {
        let data = CFData::from_buffer(encoded);
        // Safety: the source takes its own reference to the data; null options means "none".
        let raw = unsafe { CGImageSourceCreateWithData(data.as_concrete_TypeRef(), ptr::null()) };
        if raw.is_null() {
            return Err(unrecognised());
        }
        // From here the pointer has an owner that releases it exactly once.
        let mut source = Source {
            raw,
            pictures: Pictures::Still { index: 0 },
            oriented_size: [0, 0],
        };
        // A source comes back for any bytes at all; only its type says whether ImageIO knows the
        // container. Without this an unknown file would look damaged rather than unsupported.
        if unsafe { CGImageSourceGetType(source.raw) }.is_null() {
            return Err(unrecognised());
        }
        source.pictures = source.read_pictures();
        source.oriented_size = source.read_oriented_size()?;
        Ok(source)
    }

    /// `oriented_size` is the size after the camera orientation tag is applied.
    pub(crate) fn oriented_size(&self) -> [u32; 2] {
        self.oriented_size
    }

    /// `frame_count` is how many frames the image plays; one for a still image.
    pub(crate) fn frame_count(&self) -> u32 {
        match self.pictures {
            Pictures::Animation { frame_count, .. } => frame_count,
            Pictures::Still { .. } => 1,
        }
    }

    /// `repetition` is how many times the image plays: an animation as its loop count says, a
    /// still image once.
    pub(crate) fn repetition(&self) -> Repetition {
        match self.pictures {
            Pictures::Animation { repetition, .. } => repetition,
            Pictures::Still { .. } => Repetition::Once,
        }
    }

    /// `image_index` is the container's index of the image `frame` decodes from: an animation's
    /// frame, or a still image's one.
    pub(crate) fn image_index(&self, frame: u32) -> usize {
        match self.pictures {
            Pictures::Animation { .. } => frame as usize,
            Pictures::Still { index } => index,
        }
    }

    /// `frame_duration` is how long frame `index` stays on screen; zero when the file says
    /// nothing, which a caller replaces with a delay of its own.
    pub(crate) fn frame_duration(&self, index: usize) -> Duration {
        let Some(properties) = self.frame_properties(index) else {
            return Duration::ZERO;
        };
        let seconds = animation_keys().into_iter().find_map(|keys| {
            let format = nested(&properties, keys.dictionary)?;
            // The unclamped delay first: the clamped one quietly raises a very short delay to a
            // tenth of a second, a browser compatibility rule rather than what the file says.
            [keys.unclamped_delay, Some(keys.delay)]
                .into_iter()
                .flatten()
                .find_map(|key| number_in(&format, key).filter(|delay| *delay > 0.0))
        });
        seconds.map_or(Duration::ZERO, Duration::from_secs_f64)
    }

    /// `decode_scaled` decodes frame `index` no larger than `longest_side` on its longer edge.
    ///
    /// `FromImageAlways` makes this a real decode rather than a read of whatever thumbnail the
    /// file embeds, and `WithTransform` applies the orientation tag so a portrait photograph is
    /// not returned on its side. A frame of an animation arrives already composited over the
    /// frames before it, at the full canvas size, so no caller assembles one.
    pub(crate) fn decode_scaled(
        &self,
        index: usize,
        longest_side: u32,
    ) -> Result<CGImage, DecodeError> {
        let options = unsafe {
            CFDictionary::from_CFType_pairs(&[
                (
                    key(kCGImageSourceCreateThumbnailFromImageAlways),
                    CFBoolean::true_value().as_CFType(),
                ),
                (
                    key(kCGImageSourceCreateThumbnailWithTransform),
                    CFBoolean::true_value().as_CFType(),
                ),
                (
                    key(kCGImageSourceThumbnailMaxPixelSize),
                    CFNumber::from(i64::from(longest_side.max(1))).as_CFType(),
                ),
            ])
        };
        // Safety: the source is live; the returned image is owned by this call, which
        // `CGImage::from_ptr` takes over.
        let image = unsafe {
            CGImageSourceCreateThumbnailAtIndex(self.raw, index, options.as_concrete_TypeRef())
        };
        if image.is_null() {
            // The container was recognised at open, so a frame that will not decode is damaged.
            return Err(DecodeError::InvalidData(format!(
                "ImageIO could not decode frame {index}"
            )));
        }
        Ok(unsafe { CGImage::from_ptr(image) })
    }

    /// What its images are: an animation where its format's dictionary gives a loop count, which
    /// ImageIO reads for every animation, a GIF without one included; otherwise one image.
    fn read_pictures(&self) -> Pictures {
        match self.loop_count() {
            Some(count) => Pictures::Animation {
                frame_count: self.image_count().max(1),
                repetition: repetition_of(count),
            },
            None => Pictures::Still {
                index: self.primary_index(),
            },
        }
    }

    fn image_count(&self) -> u32 {
        (unsafe { CGImageSourceGetCount(self.raw) }) as u32
    }

    /// The loop count the container's animation dictionary gives, where it is an animation.
    fn loop_count(&self) -> Option<f64> {
        let properties = self.container_properties()?;
        animation_keys()
            .into_iter()
            .find_map(|keys| number_in(&nested(&properties, keys.dictionary)?, keys.loop_count))
    }

    /// The image the container names as its primary, which only a HEIF names; the first of any
    /// other, and of every container on an OS older than the call.
    fn primary_index(&self) -> usize {
        let index = imageio_function("CGImageSourceGetPrimaryImageIndex").map_or(0, |address| {
            // Safety: the symbol is ImageIO's `size_t CGImageSourceGetPrimaryImageIndex(
            // CGImageSourceRef)`, and the source is live.
            let primary: unsafe extern "C" fn(CGImageSourceRef) -> usize =
                unsafe { std::mem::transmute(address) };
            unsafe { primary(self.raw) }
        });
        if index < self.image_count() as usize {
            index
        } else {
            0
        }
    }

    fn read_oriented_size(&self) -> Result<[u32; 2], OpenError> {
        let properties = self
            .frame_properties(self.image_index(0))
            .ok_or_else(|| missing("image properties"))?;
        let width = number_in(&properties, unsafe { kCGImagePropertyPixelWidth });
        let height = number_in(&properties, unsafe { kCGImagePropertyPixelHeight });
        let (Some(width), Some(height)) = (width, height) else {
            return Err(missing("image dimensions"));
        };
        if width < 1.0 || height < 1.0 {
            return Err(DecodeError::InvalidData("empty image".into()).into());
        }
        let mut size = [width as u32, height as u32];
        // EXIF orientations 5–8 are the transposed ones.
        if number_in(&properties, unsafe { kCGImagePropertyOrientation })
            .is_some_and(|value| (5.0..=8.0).contains(&value))
        {
            size.swap(0, 1);
        }
        Ok(size)
    }

    fn container_properties(&self) -> Option<Properties> {
        let raw = unsafe { CGImageSourceCopyProperties(self.raw, ptr::null()) };
        (!raw.is_null()).then(|| unsafe { CFDictionary::wrap_under_create_rule(raw) })
    }

    fn frame_properties(&self, index: usize) -> Option<Properties> {
        // Safety: the source is live, and the dictionary is owned by this call.
        let raw = unsafe { CGImageSourceCopyPropertiesAtIndex(self.raw, index, ptr::null()) };
        (!raw.is_null()).then(|| unsafe { CFDictionary::wrap_under_create_rule(raw) })
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        // Safety: created by `CGImageSourceCreateWithData`, released exactly once.
        unsafe { CFRelease(self.raw as *const c_void) };
    }
}

/// How many times an animation with ImageIO's loop `count` plays after its first pass.
fn repetition_of(count: f64) -> Repetition {
    match count {
        // Every animated format writes zero to mean "never stop".
        count if count <= 0.0 => Repetition::Forever,
        // ImageIO reports how many times an animation plays altogether, where a file — and
        // every other decoder — counts the passes after the first.
        count if count >= 2.0 => Repetition::Times(count as u32 - 1),
        _ => Repetition::Once,
    }
}

fn unrecognised() -> OpenError {
    OpenError::Unsupported("ImageIO does not recognise the container".into())
}

fn missing(what: &str) -> OpenError {
    OpenError::Failed(DecodeError::Failed(format!(
        "ImageIO did not provide {what}"
    )))
}

fn key(name: CFStringRef) -> CFType {
    unsafe { CFString::wrap_under_get_rule(name) }.as_CFType()
}

fn number_in(properties: &Properties, key: CFStringRef) -> Option<f64> {
    let key = unsafe { CFString::wrap_under_get_rule(key) };
    let value = properties.find(&key)?;
    value.downcast::<CFNumber>()?.to_f64()
}

/// `nested` reads one format's property dictionary out of a properties dictionary.
fn nested(properties: &Properties, key: CFStringRef) -> Option<Properties> {
    let key = unsafe { CFString::wrap_under_get_rule(key) };
    let value = properties.find(&key)?;
    // A dictionary of mixed value types is not a concrete type `downcast` accepts, so the check
    // is by type identifier and the wrap is by hand.
    if value.type_of() != CFDictionary::<CFString, CFType>::type_id() {
        return None;
    }
    Some(unsafe { CFDictionary::wrap_under_get_rule(value.as_CFTypeRef() as CFDictionaryRef) })
}

/// The property keys one animated format uses, since each writes its timing under its own name.
struct AnimationKeys {
    dictionary: CFStringRef,
    loop_count: CFStringRef,
    delay: CFStringRef,
    /// Absent where the platform is too old to publish the key.
    unclamped_delay: Option<CFStringRef>,
}

/// The formats this build can read timing from. WebP's and HEIF sequences' keys are resolved from
/// the ImageIO bundle at run time so an older OS without them can still load this crate.
fn animation_keys() -> Vec<AnimationKeys> {
    let mut keys = unsafe {
        vec![
            AnimationKeys {
                dictionary: kCGImagePropertyGIFDictionary,
                loop_count: kCGImagePropertyGIFLoopCount,
                delay: kCGImagePropertyGIFDelayTime,
                unclamped_delay: Some(kCGImagePropertyGIFUnclampedDelayTime),
            },
            AnimationKeys {
                dictionary: kCGImagePropertyPNGDictionary,
                loop_count: kCGImagePropertyAPNGLoopCount,
                delay: kCGImagePropertyAPNGDelayTime,
                unclamped_delay: Some(kCGImagePropertyAPNGUnclampedDelayTime),
            },
        ]
    };
    keys.extend(
        ["WebP", "HEICS"]
            .into_iter()
            .filter_map(run_time_animation_keys),
    );
    keys
}

/// The keys of the animated format ImageIO names `format` in its keys, where this OS has them.
fn run_time_animation_keys(format: &str) -> Option<AnimationKeys> {
    let key = |name: &str| imageio_symbol(&format!("kCGImageProperty{format}{name}"));
    Some(AnimationKeys {
        dictionary: key("Dictionary")?,
        loop_count: key("LoopCount")?,
        delay: key("DelayTime")?,
        unclamped_delay: key("UnclampedDelayTime"),
    })
}

fn imageio_symbol(name: &str) -> Option<CFStringRef> {
    let bundle = imageio_bundle()?;
    let name = CFString::new(name);
    let address = unsafe {
        core_foundation_sys::bundle::CFBundleGetDataPointerForName(
            bundle.as_concrete_TypeRef(),
            name.as_concrete_TypeRef(),
        )
    };
    (!address.is_null()).then(|| unsafe { *address.cast::<CFStringRef>() })
}

/// The address of ImageIO's function `name`, where this OS has it.
fn imageio_function(name: &str) -> Option<*const c_void> {
    let bundle = imageio_bundle()?;
    let name = CFString::new(name);
    let address = unsafe {
        core_foundation_sys::bundle::CFBundleGetFunctionPointerForName(
            bundle.as_concrete_TypeRef(),
            name.as_concrete_TypeRef(),
        )
    };
    (!address.is_null()).then_some(address)
}

fn imageio_bundle() -> Option<core_foundation::bundle::CFBundle> {
    core_foundation::bundle::CFBundle::bundle_with_identifier(CFString::new("com.apple.ImageIO"))
}
