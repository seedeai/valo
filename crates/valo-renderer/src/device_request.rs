//! Opening a wgpu device to render with, for a host that has no adapter
//! policy of its own. It is a convenience: a host that brings its own device
//! hands it to the context and never comes here.
//!
//! valo runs no compute, so it renders on downlevel adapters (a GL driver,
//! WebGL2) as well as on WebGPU-compliant ones. The caller says which
//! qualify ([`FeatureLevel`]) and how to rank them (wgpu's own
//! `RequestAdapterOptions`), and the adapters are tried best first until one
//! gives a device. The ranking reads only what an adapter reports
//! ([`Candidate`]), so it is tested without a GPU; what the request asks of
//! an adapter is in [`device_descriptor`].
//!
//! The browser hands out one adapter, already chosen by the options, so on
//! the web this asks for that one and checks only its feature level.

use std::fmt;

/// `FeatureLevel` is WebGPU's `featureLevel`: which adapters qualify.
///
/// valo renders at either level. `Core` is for hosts that want what the
/// WebGPU specification guarantees, such as tests that compare pixels across
/// platforms; `Compatibility` also takes the adapters a machine may only
/// have, such as a GPU whose sole driver is GL.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FeatureLevel {
    /// `Core` admits fully WebGPU-compliant adapters only.
    Core,
    /// `Compatibility` admits downlevel adapters too.
    Compatibility,
}

/// `NoDevice` is why [`request_device`] opened no device: every adapter it
/// considered and why each gave none.
#[derive(Debug)]
pub struct NoDevice {
    /// `refused` lists the adapters that qualified, in the order they were
    /// asked for a device, then those that did not qualify. It is empty when
    /// the platform reported no adapter at all.
    pub refused: Vec<RefusedAdapter>,
    /// `no_adapter` is the browser's answer when it gave no adapter; `None`
    /// on native platforms, which enumerate.
    pub no_adapter: Option<wgpu::RequestAdapterError>,
}

/// `RefusedAdapter` is one adapter [`request_device`] could not use.
#[derive(Debug)]
pub struct RefusedAdapter {
    /// `adapter` is what the adapter reported about itself.
    pub adapter: wgpu::AdapterInfo,
    /// `refusal` is why it gave no device.
    pub refusal: Refusal,
}

/// `Refusal` is why one adapter gave no device.
#[derive(Debug)]
pub enum Refusal {
    /// `Downlevel` is an adapter that is not fully WebGPU-compliant, when
    /// [`FeatureLevel::Core`] was asked for.
    Downlevel,
    /// `NotFallback` is a hardware adapter, when the options set
    /// `force_fallback_adapter`.
    NotFallback,
    /// `SurfaceUnsupported` is an adapter that cannot present to the options'
    /// `compatible_surface`.
    SurfaceUnsupported,
    /// `Device` is an adapter that qualified but refused the device.
    Device(wgpu::RequestDeviceError),
}

/// `request_device` opens a device valo can render with.
///
/// It tries every adapter `level` admits, ranked by `options`: by power
/// preference over the adapter's type (high performance ranks discrete,
/// integrated, virtual, other; low power ranks integrated first; no
/// preference keeps the platform's order), with software adapters last —
/// or alone, when `force_fallback_adapter` is set — and, given a
/// `compatible_surface`, only adapters that can present to it. The first
/// that gives a device wins. Each is asked for its own limits rather than
/// WebGPU's defaults, which a downlevel adapter refuses, and for timestamp
/// queries when it offers them (the frame's GPU time in `RenderStats`).
/// It is also asked for mappable primary buffers when it offers them, so
/// valo writes each frame's uniforms and vertices straight into mapped
/// memory instead of through queue writes.
///
/// # Errors
///
/// [`NoDevice`] lists every adapter considered and why each gave no device.
pub async fn request_device(
    instance: &wgpu::Instance,
    options: &wgpu::RequestAdapterOptions<'_, '_>,
    level: FeatureLevel,
) -> Result<(wgpu::Adapter, wgpu::Device, wgpu::Queue), NoDevice> {
    let found = Found::of(instance, options).await?;
    let ranking = Ranking::of(&found.candidates, &found.choice, level);
    let adapters = found.adapters;
    let mut refused = Vec::new();
    for &index in &ranking.order {
        let adapter = &adapters[index];
        match adapter.request_device(&device_descriptor(adapter)).await {
            Ok((device, queue)) => return Ok((adapter.clone(), device, queue)),
            Err(error) => refused.push(RefusedAdapter {
                adapter: adapter.get_info(),
                refusal: Refusal::Device(error),
            }),
        }
    }
    refused.extend(
        ranking
            .unqualified
            .into_iter()
            .map(|(index, refusal)| RefusedAdapter {
                adapter: adapters[index].get_info(),
                refusal,
            }),
    );
    Err(NoDevice {
        refused,
        no_adapter: None,
    })
}

/// `Found` is the adapters [`request_device`] considers, what the ranking
/// reads of each, and what of the options is left for it to apply.
struct Found {
    adapters: Vec<wgpu::Adapter>,
    candidates: Vec<Candidate>,
    choice: Choice,
}

impl Found {
    /// `of` is every adapter the platform offers; the ranking applies all
    /// the options.
    #[cfg(not(target_arch = "wasm32"))]
    async fn of(
        instance: &wgpu::Instance,
        options: &wgpu::RequestAdapterOptions<'_, '_>,
    ) -> Result<Self, NoDevice> {
        let adapters = instance.enumerate_adapters(wgpu::Backends::all()).await;
        let candidates = adapters
            .iter()
            .map(|adapter| Candidate::of(adapter, options.compatible_surface))
            .collect();
        Ok(Self {
            adapters,
            candidates,
            choice: Choice::from_options(options),
        })
    }

    /// `of` is the one adapter the browser chooses by the options; it has
    /// applied them all, so the ranking only checks the level.
    #[cfg(target_arch = "wasm32")]
    async fn of(
        instance: &wgpu::Instance,
        options: &wgpu::RequestAdapterOptions<'_, '_>,
    ) -> Result<Self, NoDevice> {
        let adapter = instance
            .request_adapter(options)
            .await
            .map_err(|error| NoDevice {
                refused: Vec::new(),
                no_adapter: Some(error),
            })?;
        Ok(Self {
            candidates: vec![Candidate::of(&adapter, None)],
            adapters: vec![adapter],
            choice: Choice::default(),
        })
    }
}

/// `device_descriptor` is what [`request_device`] asks of `adapter`: its own
/// limits, and the features valo uses when it offers them.
fn device_descriptor(adapter: &wgpu::Adapter) -> wgpu::DeviceDescriptor<'static> {
    let wanted = wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::MAPPABLE_PRIMARY_BUFFERS;
    wgpu::DeviceDescriptor {
        label: Some("valo"),
        required_features: adapter.features() & wanted,
        required_limits: adapter.limits(),
        ..Default::default()
    }
}

/// `Candidate` is what the ranking reads of an adapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Candidate {
    device_type: wgpu::DeviceType,
    webgpu_compliant: bool,
    /// The adapter can present to the requested surface, or none was
    /// requested.
    presents: bool,
}

impl Candidate {
    fn of(adapter: &wgpu::Adapter, surface: Option<&wgpu::Surface<'_>>) -> Self {
        Self {
            device_type: adapter.get_info().device_type,
            webgpu_compliant: adapter.get_downlevel_capabilities().is_webgpu_compliant(),
            presents: surface.is_none_or(|surface| adapter.is_surface_supported(surface)),
        }
    }
}

/// `Choice` is the part of wgpu's adapter options the ranking applies.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Choice {
    power_preference: wgpu::PowerPreference,
    force_fallback_adapter: bool,
}

impl Choice {
    #[cfg(not(target_arch = "wasm32"))]
    fn from_options(options: &wgpu::RequestAdapterOptions<'_, '_>) -> Self {
        Self {
            power_preference: options.power_preference,
            force_fallback_adapter: options.force_fallback_adapter,
        }
    }

    /// `rank` is where an adapter of `device_type` goes: lower is tried
    /// first. Software adapters go last whatever the preference.
    fn rank(&self, device_type: wgpu::DeviceType) -> u8 {
        use wgpu::DeviceType::{Cpu, DiscreteGpu, IntegratedGpu, Other, VirtualGpu};
        use wgpu::PowerPreference::{HighPerformance, LowPower};
        match (self.power_preference, device_type) {
            (_, Cpu) => 4,
            (HighPerformance, DiscreteGpu) | (LowPower, IntegratedGpu) => 0,
            (HighPerformance, IntegratedGpu) | (LowPower, DiscreteGpu) => 1,
            (HighPerformance | LowPower, VirtualGpu) => 2,
            (HighPerformance | LowPower, Other) => 3,
            _ => 0,
        }
    }

    /// `refusal` is why `candidate` does not qualify at `level`, or `None`.
    fn refusal(&self, candidate: &Candidate, level: FeatureLevel) -> Option<Refusal> {
        if !candidate.presents {
            Some(Refusal::SurfaceUnsupported)
        } else if self.force_fallback_adapter && candidate.device_type != wgpu::DeviceType::Cpu {
            Some(Refusal::NotFallback)
        } else if level == FeatureLevel::Core && !candidate.webgpu_compliant {
            Some(Refusal::Downlevel)
        } else {
            None
        }
    }
}

/// `Ranking` is the order adapters are asked for a device in, by index into
/// the candidates, and why each of the others does not qualify.
#[derive(Debug)]
struct Ranking {
    order: Vec<usize>,
    unqualified: Vec<(usize, Refusal)>,
}

impl Ranking {
    /// `of` ranks `candidates` for `choice` at `level`: qualifying ones by
    /// [`Choice::rank`], ties in the platform's order.
    fn of(candidates: &[Candidate], choice: &Choice, level: FeatureLevel) -> Self {
        let mut order = Vec::new();
        let mut unqualified = Vec::new();
        for (index, candidate) in candidates.iter().enumerate() {
            match choice.refusal(candidate, level) {
                Some(refusal) => unqualified.push((index, refusal)),
                None => order.push(index),
            }
        }
        order.sort_by_key(|&index| choice.rank(candidates[index].device_type));
        Self { order, unqualified }
    }
}

/// `test_device` is the device valo's GPU tests render with, as the
/// harness opens it: core feature level, high performance first.
#[cfg(test)]
pub(crate) fn test_device() -> Option<(wgpu::Adapter, wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let options = wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    };
    pollster::block_on(request_device(&instance, &options, FeatureLevel::Core)).ok()
}

impl fmt::Display for NoDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(error) = &self.no_adapter {
            return write!(f, "no adapter: {error}");
        }
        if self.refused.is_empty() {
            return write!(f, "no adapter found");
        }
        write!(f, "no adapter gave a device:")?;
        for refused in &self.refused {
            let adapter = &refused.adapter;
            write!(f, " {} ({:?}): ", adapter.name, adapter.backend)?;
            match &refused.refusal {
                Refusal::Downlevel => write!(f, "not fully WebGPU-compliant;")?,
                Refusal::NotFallback => write!(f, "not a software adapter;")?,
                Refusal::SurfaceUnsupported => write!(f, "cannot present to the surface;")?,
                Refusal::Device(error) => write!(f, "refused the device: {error};")?,
            }
        }
        Ok(())
    }
}

impl std::error::Error for NoDevice {}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::DeviceType::{Cpu, DiscreteGpu, IntegratedGpu, Other, VirtualGpu};

    fn adapter(device_type: wgpu::DeviceType, webgpu_compliant: bool) -> Candidate {
        Candidate {
            device_type,
            webgpu_compliant,
            presents: true,
        }
    }

    fn choice(power_preference: wgpu::PowerPreference) -> Choice {
        Choice {
            power_preference,
            force_fallback_adapter: false,
        }
    }

    /// One adapter of every type, compliant, in an order no preference
    /// ranks them in.
    fn every_type() -> Vec<Candidate> {
        [Cpu, Other, VirtualGpu, IntegratedGpu, DiscreteGpu]
            .map(|device_type| adapter(device_type, true))
            .to_vec()
    }

    fn types_in_order(candidates: &[Candidate], ranking: &Ranking) -> Vec<wgpu::DeviceType> {
        ranking
            .order
            .iter()
            .map(|&index| candidates[index].device_type)
            .collect()
    }

    #[test]
    fn high_performance_ranks_discrete_integrated_virtual_other_then_software() {
        let candidates = every_type();
        let ranking = Ranking::of(
            &candidates,
            &choice(wgpu::PowerPreference::HighPerformance),
            FeatureLevel::Core,
        );
        assert_eq!(
            types_in_order(&candidates, &ranking),
            [DiscreteGpu, IntegratedGpu, VirtualGpu, Other, Cpu]
        );
    }

    #[test]
    fn low_power_ranks_integrated_first() {
        let candidates = every_type();
        let ranking = Ranking::of(
            &candidates,
            &choice(wgpu::PowerPreference::LowPower),
            FeatureLevel::Core,
        );
        assert_eq!(
            types_in_order(&candidates, &ranking),
            [IntegratedGpu, DiscreteGpu, VirtualGpu, Other, Cpu]
        );
    }

    /// Without a preference the platform's order stands, but a software
    /// adapter still waits for the hardware ones.
    #[test]
    fn no_preference_keeps_the_platform_order_with_software_last() {
        let candidates = every_type();
        let ranking = Ranking::of(
            &candidates,
            &choice(wgpu::PowerPreference::None),
            FeatureLevel::Core,
        );
        assert_eq!(
            types_in_order(&candidates, &ranking),
            [Other, VirtualGpu, IntegratedGpu, DiscreteGpu, Cpu]
        );
    }

    /// A virtual machine's case: a downlevel GL adapter beside a compliant
    /// software one. Core takes the software adapter; compatibility tries
    /// the hardware one first.
    #[test]
    fn core_passes_over_a_downlevel_adapter() {
        let candidates = vec![adapter(IntegratedGpu, false), adapter(Cpu, true)];
        let preference = choice(wgpu::PowerPreference::HighPerformance);
        let core = Ranking::of(&candidates, &preference, FeatureLevel::Core);
        assert_eq!(core.order, [1]);
        assert!(matches!(core.unqualified[..], [(0, Refusal::Downlevel)]));
        let compatibility = Ranking::of(&candidates, &preference, FeatureLevel::Compatibility);
        assert_eq!(compatibility.order, [0, 1]);
        assert!(compatibility.unqualified.is_empty());
    }

    #[test]
    fn forcing_the_fallback_admits_software_adapters_only() {
        let candidates = every_type();
        let forced = Choice {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: true,
        };
        let ranking = Ranking::of(&candidates, &forced, FeatureLevel::Core);
        assert_eq!(types_in_order(&candidates, &ranking), [Cpu]);
        assert_eq!(ranking.unqualified.len(), 4);
        assert!(ranking
            .unqualified
            .iter()
            .all(|(_, refusal)| matches!(refusal, Refusal::NotFallback)));
    }

    #[test]
    fn an_adapter_that_cannot_present_does_not_qualify() {
        let candidates = vec![
            Candidate {
                presents: false,
                ..adapter(DiscreteGpu, true)
            },
            adapter(IntegratedGpu, true),
        ];
        let ranking = Ranking::of(
            &candidates,
            &choice(wgpu::PowerPreference::HighPerformance),
            FeatureLevel::Compatibility,
        );
        assert_eq!(ranking.order, [1]);
        assert!(matches!(
            ranking.unqualified[..],
            [(0, Refusal::SurfaceUnsupported)]
        ));
    }

    /// The harness's request, without the harness: where the platform has a
    /// compliant adapter, `Core` opens a device on one.
    #[test]
    fn core_opens_a_device_on_a_compliant_adapter() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let compliant = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()))
            .iter()
            .any(|adapter| adapter.get_downlevel_capabilities().is_webgpu_compliant());
        match test_device() {
            Some((adapter, _, _)) => {
                assert!(adapter.get_downlevel_capabilities().is_webgpu_compliant());
            }
            None => {
                assert!(
                    !compliant,
                    "a compliant adapter is there but gave no device"
                );
                eprintln!("SKIP core_opens_a_device_on_a_compliant_adapter: no compliant adapter");
            }
        }
    }
}
