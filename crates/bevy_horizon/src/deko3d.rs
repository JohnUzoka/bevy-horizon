//! Bevy's renderer on deko3d, the Switch's own GPU API, instead of Vulkan and NVK.
//!
//! ```ignore
//! DefaultPlugins.set(RenderPlugin { render_creation: bevy_horizon::deko3d::render_creation(), ..default() })
//! ```
//! This creates what bevy_render's automatic setup would, on crates/wgpu_deko3d's one adapter: a
//! device with every feature the adapter has and its limits. docs/deko3d.md lists what deko3d
//! supports and what it doesn't.
use std::sync::Arc;

use bevy_render::renderer::{RenderAdapter, RenderAdapterInfo, RenderDevice, RenderInstance, RenderQueue, WgpuWrapper};
use bevy_render::settings::RenderCreation;
use wgpu_deko3d::wgpu;

pub fn render_creation() -> RenderCreation {
    let instance = wgpu_deko3d::instance();
    let adapter = bevy_tasks::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        .expect("deko3d offers one adapter");
    let info = adapter.get_info();
    tracing::info!("bevy_horizon: rendering through {} ({})", info.name, info.driver_info);
    let (device, queue) = bevy_tasks::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("bevy_horizon deko3d"),
        required_features: adapter.features(),
        required_limits: adapter.limits(),
        ..Default::default()
    }))
    .expect("deko3d device");
    RenderCreation::manual(
        RenderDevice::from(device),
        RenderQueue(Arc::new(WgpuWrapper::new(queue))),
        RenderAdapterInfo(WgpuWrapper::new(info)),
        RenderAdapter(Arc::new(WgpuWrapper::new(adapter))),
        RenderInstance(Arc::new(WgpuWrapper::new(instance))),
    )
}
