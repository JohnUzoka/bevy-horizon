//! Paces GPU uploads, for a console whose GPU memory comes out of the game's own heap.
//!
//! On the Switch, NVK takes every GPU buffer and image from the heap the game's malloc allocates
//! from (mesa-switch `nouveau_horizon_memory.c`), each as one contiguous block. An
//! upload is in that heap twice for a while: wgpu copies the data into a staging buffer, and frees
//! that buffer only once the GPU has finished the submission that used it. Bevy uploads every
//! mesh that reaches the render world in that same frame: its mesh allocator (bevy_render
//! `mesh/allocator.rs`, `allocate_and_free_meshes`) copies them all to the GPU, and
//! `RenderAssetBytesPerFrame` paces only `prepare_assets`, which runs after it. A level that adds a
//! few hundred MiB of meshes at once asks for that much again in staging, in one frame, as large
//! blocks. Once loading has freed memory in the middle of the heap, only the heap's untouched top
//! has room for blocks that size (NVK takes GPU memory from there first, then from malloc's holes;
//! bevy-horizon's `patches/mesa-switch/`), and when neither has room NVK fails with
//! `VK_ERROR_OUT_OF_HOST_MEMORY`, which wgpu reports as "Out of Memory" (Skate 3; see
//! docs/platform-notes.md).
//!
//! [`HorizonUploadPlugin`], which `HorizonPlugin` adds with this feature, does three things:
//!
//! - It hands Bevy's mesh allocator at most [`HorizonUploadSettings::mesh_bytes_per_frame`] of new
//!   meshes a frame, oldest first, and keeps the rest in the render world for later frames. To the
//!   rest of Bevy a held mesh looks like one that hasn't loaded yet. When a held mesh goes, the
//!   plugin marks the `Mesh3d` and `Mesh2d` components that use it as changed. Bevy then extracts
//!   those entities again, so its GPU mesh building picks up where the mesh went: it records that
//!   when it extracts an entity, and Bevy itself extracts them again only for a modified mesh.
//! - After a frame that uploaded at least [`HorizonUploadSettings::reclaim_after_bytes`], it waits
//!   for the GPU to finish that frame, so wgpu frees the frame's staging buffers before the next
//!   frame makes more.
//! - If the game hasn't set Bevy's `RenderAssetBytesPerFrame` (images and the other render assets),
//!   it sets it to [`HorizonUploadSettings::default_asset_bytes_per_frame`].
//!
//! From Bevy (MIT OR Apache-2.0; CREDITS.md): the pacing follows bevy_render's
//! `RenderAssetBytesPerFrameLimiter` (`render_asset.rs`), a byte budget a frame that always lets
//! at least one asset through however large, and marking `Mesh3d` changed is what bevy_mesh's
//! `mark_3d_meshes_as_changed_if_their_assets_changed` (`components.rs`) does for a modified mesh.

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use bevy_app::{App, Last, Plugin};
use bevy_asset::AssetId;
use bevy_ecs::prelude::*;
use bevy_mesh::{Mesh, Mesh2d, Mesh3d};
use bevy_render::mesh::RenderMesh;
use bevy_render::mesh::allocator::{MeshAllocator, allocate_and_free_meshes};
use bevy_render::render_asset::{
    ExtractedAssets, RenderAssetBytesPerFrame, RenderAssetBytesPerFrameLimiter,
    reset_render_asset_bytes_per_frame,
};
use bevy_render::render_resource::PollType;
use bevy_render::renderer::RenderDevice;
use bevy_render::{Render, RenderApp, RenderSystems};

const MIB: usize = 1 << 20;

/// How long [`HorizonUploadPlugin`] waits for the GPU after a frame that uploaded a lot.
const RECLAIM_TIMEOUT: Duration = Duration::from_secs(10);

/// Frames without a GPU wait before the log sums up the waits.
const QUIET_FRAMES: u64 = 60;

/// How [`HorizonUploadPlugin`] paces uploads. To change it, insert it into the main app before
/// `App::run`; the plugin copies it into the render world when the app starts.
#[derive(Resource, Clone, Debug)]
pub struct HorizonUploadSettings {
    /// The most bytes of new meshes (vertex plus index data) handed to Bevy's mesh allocator in a
    /// frame. The rest wait in the render world, oldest first; at least one goes each frame however
    /// large. A mesh already on the GPU that changes is never held, since Bevy frees its old copy
    /// in the frame the new one arrives. `None` hands every mesh over at once, as Bevy does.
    /// Default: 32 MiB.
    pub mesh_bytes_per_frame: Option<usize>,
    /// After a frame that uploaded at least this much (the meshes handed to the allocator plus
    /// what Bevy's `RenderAssetBytesPerFrameLimiter` counted), wait up to 10 s for the GPU to
    /// finish it, so wgpu frees the frame's staging buffers before the next frame makes more.
    /// `None` never waits. Default: 8 MiB.
    pub reclaim_after_bytes: Option<usize>,
    /// Bevy's `RenderAssetBytesPerFrame`, the most bytes of images and other render assets
    /// prepared in a frame, for a game that hasn't set one when the app starts. A game that wants
    /// no limit sets `usize::MAX`. `None` leaves Bevy's default, no limit. Default: 64 MiB.
    pub default_asset_bytes_per_frame: Option<usize>,
}

impl Default for HorizonUploadSettings {
    fn default() -> Self {
        Self {
            mesh_bytes_per_frame: Some(32 * MIB),
            reclaim_after_bytes: Some(8 * MIB),
            default_asset_bytes_per_frame: Some(64 * MIB),
        }
    }
}

/// Paces GPU uploads; see the [module docs](self). `HorizonPlugin` adds it when bevy_horizon's
/// `render` (or `render_0_18`) feature is on. Does nothing without a render app.
#[derive(Default)]
pub struct HorizonUploadPlugin;

impl Plugin for HorizonUploadPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HorizonUploadSettings>()
            .init_resource::<ReleasedMeshes>()
            .add_systems(Last, mark_released_meshes_changed);
    }

    // The render app and Bevy's own resources exist by now, whichever order the game added
    // `HorizonPlugin` and `DefaultPlugins` in.
    fn finish(&self, app: &mut App) {
        let settings = app.world().resource::<HorizonUploadSettings>().clone();
        if let Some(bytes) = settings.default_asset_bytes_per_frame
            && let Some(mut limit) = app.world_mut().get_resource_mut::<RenderAssetBytesPerFrame>()
            && limit.max_bytes.is_none()
        {
            limit.max_bytes = Some(bytes);
        }
        let asset_limit = app
            .world()
            .get_resource::<RenderAssetBytesPerFrame>()
            .and_then(|limit| limit.max_bytes);
        let released = app.world().resource::<ReleasedMeshes>().clone();
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .insert_resource(settings.clone())
            .insert_resource(released)
            .init_resource::<UploadState>()
            .add_systems(
                Render,
                (
                    pace_mesh_uploads
                        .in_set(RenderSystems::PrepareAssets)
                        .before(allocate_and_free_meshes),
                    reclaim_upload_staging
                        .in_set(RenderSystems::Cleanup)
                        .before(reset_render_asset_bytes_per_frame),
                ),
            );
        tracing::info!(
            "[Upload] GPU uploads a frame: new meshes {}, other render assets {} (RenderAssetBytesPerFrame); waits for the GPU after frames that upload {}",
            limit_text(settings.mesh_bytes_per_frame),
            limit_text(asset_limit),
            match settings.reclaim_after_bytes {
                Some(bytes) => format!("{:.1} MiB or more", mib(bytes)),
                None => "never".to_string(),
            },
        );
    }
}

/// Meshes the render world held for at least a frame and has now handed to the allocator. The
/// main world marks the entities that use them as changed.
#[derive(Resource, Clone, Default)]
struct ReleasedMeshes(Arc<Mutex<Vec<AssetId<Mesh>>>>);

struct HeldMesh {
    id: AssetId<Mesh>,
    mesh: Mesh,
    bytes: usize,
    /// The render frame the mesh was extracted in.
    frame: u64,
}

/// Meshes held since the queue was last empty, for the log.
struct HoldRound {
    start_frame: u64,
    start: Instant,
    meshes: usize,
    bytes: usize,
}

#[derive(Resource, Default)]
struct UploadState {
    frame: u64,
    held: VecDeque<HeldMesh>,
    /// Bytes of meshes handed to the mesh allocator this frame.
    mesh_bytes_sent: usize,
    round: Option<HoldRound>,
    /// GPU waits since the log last summed them up.
    waits: u32,
    wait_time: Duration,
    waited_bytes: usize,
    last_wait_frame: u64,
    poll_error_logged: bool,
}

fn mib(bytes: usize) -> f64 {
    bytes as f64 / MIB as f64
}

fn limit_text(limit: Option<usize>) -> String {
    match limit {
        Some(bytes) => format!("at most {:.1} MiB", mib(bytes)),
        None => "no limit".to_string(),
    }
}

/// What the mesh allocator copies to the GPU for `mesh`.
fn mesh_bytes(mesh: &Mesh) -> usize {
    mesh.get_vertex_buffer_size() + mesh.get_index_buffer_bytes().map_or(0, <[u8]>::len)
}

/// Runs just before Bevy's `allocate_and_free_meshes` and decides which of this frame's and the
/// held meshes it gets.
fn pace_mesh_uploads(
    settings: Res<HorizonUploadSettings>,
    mut state: ResMut<UploadState>,
    released: Res<ReleasedMeshes>,
    extracted: Option<ResMut<ExtractedAssets<RenderMesh>>>,
    allocator: Option<Res<MeshAllocator>>,
) {
    let state = &mut *state;
    state.frame += 1;
    state.mesh_bytes_sent = 0;
    let frame = state.frame;
    let Some(mut extracted) = extracted else {
        return;
    };
    let assets = &mut *extracted;

    // The game dropped these since, or they were extracted again and the new copy replaces them.
    if !state.held.is_empty() {
        state
            .held
            .retain(|held| !assets.removed.contains(&held.id) && !assets.added.contains(&held.id));
    }

    let budget = settings.mesh_bytes_per_frame.unwrap_or(usize::MAX);
    let mut sent = 0usize;
    let mut send = Vec::with_capacity(assets.extracted.len());
    for (id, mesh) in assets.extracted.drain(..) {
        let bytes = mesh_bytes(&mesh);
        let on_gpu = allocator.as_deref().is_some_and(|allocator| {
            allocator.mesh_vertex_slice(&id).is_some() || allocator.mesh_index_slice(&id).is_some()
        });
        if on_gpu || assets.modified.contains(&id) {
            sent += bytes;
            send.push((id, mesh));
        } else {
            state.held.push_back(HeldMesh { id, mesh, bytes, frame });
        }
    }

    let mut late_meshes = Vec::new();
    let mut late_bytes = 0usize;
    let mut first = true;
    while let Some(next) = state.held.front() {
        if !first && sent.saturating_add(next.bytes) > budget {
            break;
        }
        first = false;
        let Some(held) = state.held.pop_front() else {
            break;
        };
        sent += held.bytes;
        if held.frame != frame {
            late_meshes.push(held.id);
            late_bytes += held.bytes;
        }
        assets.added.insert(held.id);
        send.push((held.id, held.mesh));
    }
    // As far as the rest of Bevy can tell, the meshes still held weren't extracted this frame.
    for held in state.held.iter().filter(|held| held.frame == frame) {
        assets.added.remove(&held.id);
    }
    assets.extracted = send;
    state.mesh_bytes_sent = sent;

    if !late_meshes.is_empty() {
        let count = late_meshes.len();
        released.0.lock().unwrap_or_else(PoisonError::into_inner).extend(late_meshes);
        if let Some(round) = &mut state.round {
            round.meshes += count;
            round.bytes += late_bytes;
        }
    }
    if !state.held.is_empty() && state.round.is_none() {
        let waiting: usize = state.held.iter().map(|held| held.bytes).sum();
        tracing::info!(
            "[Upload] holding {} new meshes ({:.1} MiB) for later frames: Bevy's mesh allocator gets at most {:.1} MiB of new meshes a frame (HorizonUploadSettings)",
            state.held.len(),
            mib(waiting),
            mib(budget),
        );
        state.round = Some(HoldRound { start_frame: frame, start: Instant::now(), meshes: 0, bytes: 0 });
    }
    if state.held.is_empty()
        && let Some(round) = state.round.take()
    {
        tracing::info!(
            "[Upload] the last held mesh went to the GPU: {} meshes ({:.1} MiB) waited, over {} frames ({:.1} s)",
            round.meshes,
            mib(round.bytes),
            frame - round.start_frame,
            round.start.elapsed().as_secs_f32(),
        );
    }
}

/// Runs at the end of the render frame, after its submission, while Bevy's limiter still holds the
/// frame's count.
fn reclaim_upload_staging(
    settings: Res<HorizonUploadSettings>,
    mut state: ResMut<UploadState>,
    limiter: Option<Res<RenderAssetBytesPerFrameLimiter>>,
    device: Option<Res<RenderDevice>>,
) {
    let state = &mut *state;
    let (Some(threshold), Some(device)) = (settings.reclaim_after_bytes, device) else {
        return;
    };
    let uploaded = limiter
        .map_or(0, |limiter| limiter.bytes_written.load(Ordering::Relaxed))
        .saturating_add(state.mesh_bytes_sent);
    if uploaded < threshold {
        if state.waits > 0 && state.frame >= state.last_wait_frame + QUIET_FRAMES {
            tracing::info!(
                "[Upload] waited for the GPU after {} frames that uploaded {:.1} MiB between them, {} ms in all, so wgpu could free their staging buffers",
                state.waits,
                mib(state.waited_bytes),
                state.wait_time.as_millis(),
            );
            state.waits = 0;
            state.wait_time = Duration::ZERO;
            state.waited_bytes = 0;
        }
        return;
    }
    let start = Instant::now();
    let result = device.poll(PollType::Wait {
        submission_index: None,
        timeout: Some(RECLAIM_TIMEOUT),
    });
    state.wait_time += start.elapsed();
    state.waits += 1;
    state.waited_bytes = state.waited_bytes.saturating_add(uploaded);
    state.last_wait_frame = state.frame;
    if let Err(error) = result
        && !state.poll_error_logged
    {
        state.poll_error_logged = true;
        tracing::warn!("[Upload] waiting for the GPU to free upload buffers failed: {error}");
    }
}

/// Marks the `Mesh3d` and `Mesh2d` components that use a mesh the render world held and has now
/// handed to the allocator as changed, so Bevy extracts those entities again.
fn mark_released_meshes_changed(
    released: Res<ReleasedMeshes>,
    mut meshes_3d: Query<&mut Mesh3d>,
    mut meshes_2d: Query<&mut Mesh2d>,
) {
    let ids: HashSet<AssetId<Mesh>> = {
        let mut list = released.0.lock().unwrap_or_else(PoisonError::into_inner);
        if list.is_empty() {
            return;
        }
        list.drain(..).collect()
    };
    let mut marked = 0usize;
    for mut mesh in &mut meshes_3d {
        if ids.contains(&mesh.0.id()) {
            mesh.set_changed();
            marked += 1;
        }
    }
    for mut mesh in &mut meshes_2d {
        if ids.contains(&mesh.0.id()) {
            mesh.set_changed();
            marked += 1;
        }
    }
    tracing::info!(
        "[Upload] {} held meshes reached the GPU; marked the {marked} entities that use them as changed",
        ids.len(),
    );
}
