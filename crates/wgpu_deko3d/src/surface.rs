//! Presentation through a deko3d swapchain on the Horizon NWindow.
use std::{
    any::Any,
    borrow::Borrow,
    ffi::c_void,
    ptr::NonNull,
    sync::{Arc, Mutex},
};

use wgpu_hal as hal;
use wgpu_types as wgt;

use super::{dk, ffi, Api, Context, Fence, Handle, NativeTexture, Texture};

const FORMATS: [wgt::TextureFormat; 2] = [wgt::TextureFormat::Rgba8UnormSrgb, wgt::TextureFormat::Rgba8Unorm];

#[derive(Debug)]
struct Swapchain {
    handle: Handle,
    textures: Vec<Texture>,
    _ctx: Context,
}

impl Drop for Swapchain {
    fn drop(&mut self) {
        // Waits for the queue to go idle, so no submitted work still renders to the images.
        unsafe { ffi::dkp_swapchain_free(self.handle.ptr()) };
    }
}

#[derive(Debug)]
pub struct Surface {
    window: NonNull<c_void>,
    swapchain: Mutex<Option<Swapchain>>,
}

// The NWindow is the process's default window; libnx serializes access to it.
unsafe impl Send for Surface {}
unsafe impl Sync for Surface {}

#[derive(Debug)]
pub struct SurfaceTexture {
    texture: Texture,
    slot: i32,
    swapchain: NonNull<c_void>,
}

unsafe impl Send for SurfaceTexture {}
unsafe impl Sync for SurfaceTexture {}

impl hal::DynResource for SurfaceTexture {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

impl hal::DynSurfaceTexture for SurfaceTexture {}

impl Borrow<Texture> for SurfaceTexture {
    fn borrow(&self) -> &Texture {
        &self.texture
    }
}

impl Borrow<dyn hal::DynTexture> for SurfaceTexture {
    fn borrow(&self) -> &dyn hal::DynTexture {
        &self.texture
    }
}

impl Surface {
    pub(super) fn new(window: NonNull<c_void>) -> Self {
        Self { window, swapchain: Mutex::new(None) }
    }

    pub(super) fn capabilities(&self) -> hal::SurfaceCapabilities {
        let (mut width, mut height) = (0, 0);
        let rc = unsafe { ffi::nwindowGetDimensions(self.window.as_ptr(), &mut width, &mut height) };
        hal::SurfaceCapabilities {
            #[cfg(not(feature = "wgpu-30"))]
            formats: FORMATS.to_vec(),
            // wgpu-hal 30 lists each format's color spaces; the compositor shows sRGB.
            #[cfg(feature = "wgpu-30")]
            formats: FORMATS
                .iter()
                .map(|&format| wgt::SurfaceFormatCapabilities { format, color_spaces: wgt::SurfaceColorSpaces::SRGB })
                .collect(),
            maximum_frame_latency: 1..=2,
            current_extent: (rc == 0 && width > 0).then_some(wgt::Extent3d { width, height, depth_or_array_layers: 1 }),
            usage: wgt::TextureUses::COLOR_TARGET | wgt::TextureUses::COPY_SRC | wgt::TextureUses::COPY_DST,
            present_modes: vec![wgt::PresentMode::Fifo],
            composite_alpha_modes: vec![wgt::CompositeAlphaMode::Opaque],
        }
    }

    pub(super) fn present(&self, texture: SurfaceTexture) -> Result<(), hal::SurfaceError> {
        if !unsafe { ffi::dkp_present(texture.swapchain.as_ptr(), texture.slot) } {
            return Err(hal::SurfaceError::Lost);
        }
        super::frame_presented();
        Ok(())
    }
}

impl hal::Surface for Surface {
    type A = Api;

    unsafe fn configure(&self, device: &Context, config: &hal::SurfaceConfiguration) -> Result<(), hal::SurfaceError> {
        let mut swapchain = self.swapchain.lock().unwrap();
        *swapchain = None;
        let format = match config.format {
            wgt::TextureFormat::Rgba8UnormSrgb => dk::DkImageFormat_RGBA8_Unorm_sRGB,
            wgt::TextureFormat::Rgba8Unorm => dk::DkImageFormat_RGBA8_Unorm,
            _ => return Err(hal::SurfaceError::Other("deko3d swapchains are RGBA8")),
        };
        let size = wgt::Extent3d { width: config.extent.width, height: config.extent.height, depth_or_array_layers: 1 };
        let count = (config.maximum_frame_latency + 1).clamp(2, 3);
        let mut textures = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let desc = ffi::TextureDesc {
                ty: dk::DkImageType_2D,
                format,
                flags: dk::DkImageFlags_UsageRender | dk::DkImageFlags_UsagePresent | dk::DkImageFlags_HwCompression,
                width: size.width,
                height: size.height,
                depth: 1,
                mips: 1,
                samples: 1,
            };
            let handle = Handle::new(unsafe { ffi::dkp_texture(device.ptr(), &desc) })
                .ok_or(hal::SurfaceError::Device(hal::DeviceError::OutOfMemory))?;
            textures.push(Texture(Arc::new(NativeTexture {
                handle, format: config.format, dimension: wgt::TextureDimension::D2, size, mips: 1, samples: 1,
            })));
        }
        let raw: Vec<*mut c_void> = textures.iter().map(Texture::ptr).collect();
        // Cleared to black first: Bevy presents its first frame before drawing to it.
        let handle = Handle::new(unsafe { ffi::dkp_swapchain(device.ptr(), self.window.as_ptr(), raw.as_ptr(), count, size.width, size.height) })
            .ok_or(hal::SurfaceError::Lost)?;
        unsafe { ffi::dkp_swapchain_interval(handle.ptr(), 1) };
        *swapchain = Some(Swapchain { handle, textures, _ctx: device.clone() });
        Ok(())
    }

    unsafe fn unconfigure(&self, _: &Context) {
        *self.swapchain.lock().unwrap() = None;
    }

    unsafe fn acquire_texture(&self, _timeout: Option<std::time::Duration>, _fence: &Fence) -> Result<hal::AcquiredSurfaceTexture<Api>, hal::SurfaceError> {
        let guard = self.swapchain.lock().unwrap();
        let swapchain = guard.as_ref().ok_or(hal::SurfaceError::Outdated)?;
        // Blocks until the compositor releases an image (vsync pacing); the queue waits for it on the GPU.
        let slot = unsafe { ffi::dkp_acquire(swapchain.handle.ptr()) };
        let texture = usize::try_from(slot).ok().and_then(|s| swapchain.textures.get(s)).ok_or(hal::SurfaceError::Lost)?;
        Ok(hal::AcquiredSurfaceTexture {
            texture: SurfaceTexture { texture: texture.clone(), slot, swapchain: swapchain.handle.0 },
            suboptimal: false,
        })
    }

    unsafe fn discard_texture(&self, texture: SurfaceTexture) {
        // NWindow needs the buffer back; queueing it shows the previous contents.
        let _ = self.present(texture);
    }
}
