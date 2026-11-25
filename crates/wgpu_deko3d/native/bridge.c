// Native side of the deko3d wgpu-hal backend: thin wrappers over deko3d taking plain structs.
// wgpu-core validates the API usage; these functions keep only cheap structural checks.
#include <deko3d.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <switch.h>

enum {
    command_chunk_size = 16 * DK_MEMBLOCK_ALIGNMENT,
    zero_block_size = 16 * DK_MEMBLOCK_ALIGNMENT,
    // A single dkCmdBufCopyBuffer command fits one copy-engine launch at this size.
    copy_chunk_size = 0x3ffffc,
    // Descriptor heaps, bound once per command list. Sampler ids are 12 bits in a texture handle.
    image_descriptor_count = 4096,
    sampler_descriptor_count = 1024,
    // The GPU reads commands through a ring of GPFIFO_QUEUE_SIZE (0x800) gpfifo entries, which
    // libnx's GPU channel fills and kicks off. deko3d appends a command list's entries to the
    // channel at submission and kicks them off only when it is nearly full or the queue is
    // flushed, and nothing waits for the ring to have room: a kickoff with more entries than the
    // ring has free fails (LIBNX ABORT 2359-0001). So recordings are cut into segments of about
    // segment_entries entries; dkp_submit kicks them off in batches of at most
    // gpfifo_batch_entries, each with a fence, and before a batch waits until the kickoffs still
    // running leave room for it within ring_budget. Entries are estimated from the commands that
    // end one in deko3d (maxwell/gpu_base.cpp, gpu_3d_base.cpp, gpu_compute.cpp, dk_cmdbuf.cpp),
    // generously; the budget leaves half the ring for what the estimates miss.
    segment_entries = 128,
    kick_slots = 16,
};
// See above; dkp_set_ring_budget overrides them for measurements.
static uint32_t gpfifo_batch_entries = 384, ring_budget = 1024;

void dkp_set_ring_budget(uint32_t budget, uint32_t batch) {
    if (batch >= segment_entries && budget >= batch && budget <= 2000) {
        ring_budget = budget;
        gpfifo_batch_entries = batch;
    }
}

// Retired command-memory chunks kept for reuse. 0 restores per-encoder create/destroy.
#ifndef DKP_CHUNK_POOL
#define DKP_CHUNK_POOL 64
#endif
static const uint32_t chunk_pool_limit = DKP_CHUNK_POOL;

// CPU-written memory (staging, readback, uploads): the GPU bypasses its L2, so neither side
// needs cache maintenance. GPU-only memory is GPU cached; the CPU never touches it.
static const uint32_t mappable_flags = DkMemBlockFlags_CpuUncached | DkMemBlockFlags_GpuUncached;
static const uint32_t gpu_flags = DkMemBlockFlags_CpuUncached | DkMemBlockFlags_GpuCached;

// Benchmark instrumentation: call counts and accumulated ticks per native operation. The kinds
// from stat_timed on count an amount instead of ticks.
enum { stat_block_create, stat_block_destroy, stat_cmdbuf_create, stat_cmd_free,
       stat_submit, stat_fence_poll, stat_fence_wait,
       stat_ring_wait,      // dkp_submit waiting for the GPU to free gpfifo ring room
       stat_queue_commands, // dkQueueSubmitCommands (deko3d may wait for its own command ring)
       stat_kick,           // a kickoff's fence and dkQueueFlush
       stat_queue_lock,     // waiting for the queue lock
       stat_acquire,        // dkSwapchainAcquireImage: the compositor handing back an image
       stat_present,        // dkQueuePresentImage
       stat_wait_idle,
       stat_timed,
       stat_entries = stat_timed, // estimated gpfifo entries per submission
       stat_count };
static uint64_t stat_calls[stat_count], stat_ticks[stat_count];

static void stat_add(int kind, uint64_t amount) {
    __atomic_fetch_add(&stat_calls[kind], 1, __ATOMIC_RELAXED);
    __atomic_fetch_add(&stat_ticks[kind], amount, __ATOMIC_RELAXED);
}

static void stat_end(int kind, uint64_t started) {
    stat_add(kind, armGetSystemTick() - started);
}

uint32_t dkp_chunk_pool_limit(void) {
    return chunk_pool_limit;
}

// Writes {calls, ns (or amount)} per kind, then resets the counters.
void dkp_stats(uint64_t *out) {
    for (int i = 0; i < stat_count; ++i) {
        out[2 * i] = __atomic_exchange_n(&stat_calls[i], 0, __ATOMIC_RELAXED);
        const uint64_t amount = __atomic_exchange_n(&stat_ticks[i], 0, __ATOMIC_RELAXED);
        out[2 * i + 1] = i < stat_timed ? armTicksToNs(amount) : amount;
    }
}

typedef struct PendingFence {
    struct PendingFence *next;
    uint64_t value;
    DkFence native;
} PendingFence;

typedef struct CommandChunk {
    struct CommandChunk *previous;
    DkMemBlock memory;
} CommandChunk;

// Buffers up to 1 MiB are slots in shared, size-classed blocks: wgpu-core makes a staging
// buffer per queue.write_buffer, and a memory block per buffer costs ~0.3 ms to map and unmap.
enum { slot_min_shift = 8, slot_max_shift = 20, slot_classes = slot_max_shift - slot_min_shift + 1,
       slab_size = 0x10000,
       arena_class = 16 - slot_min_shift }; // 64 KiB upload arenas from the mappable pool

typedef struct Slot {
    struct Slot *next;
    DkMemBlock memory;
    uint32_t offset;
} Slot;

// A backing block and its slots; kept mapped until the context closes.
typedef struct Slab {
    struct Slab *next;
    DkMemBlock memory;
    Slot *slots;
} Slab;

typedef struct {
    DkDevice device;
    DkQueue queue;
    Mutex queue_lock;
    DkMemBlock zeroes;
    PendingFence *orphaned_fences;
    bool queue_lost;
    // Encoders record and resources are made without the queue lock, so these have their own.
    Mutex alloc_lock;
    CommandChunk *free_chunks;
    uint32_t free_count;
    DkMemBlock descriptors; // image descriptors, then sampler descriptors
    DkImageDescriptor *images;
    DkSamplerDescriptor *samplers;
    uint32_t free_images[image_descriptor_count], image_top;
    uint32_t free_samplers[sampler_descriptor_count], sampler_top;
    Slot *free_slots[2][slot_classes]; // [mappable][size class]
    Slab *slabs;
    // Kickoffs whose gpfifo entries may still be on the GPU's ring, oldest first (queue_lock).
    struct {
        DkFence fence;
        uint32_t entries;
    } kicks[kick_slots];
    uint32_t kick_head, kick_count, kick_entries;
} Context;

static void lock_queue(Context *ctx) {
    const uint64_t started = armGetSystemTick();
    mutexLock(&ctx->queue_lock);
    stat_end(stat_queue_lock, started);
}

typedef struct {
    Context *ctx;
    PendingFence *head, *tail;
    uint64_t submitted, completed;
} Fence;

typedef struct {
    Context *ctx;
    DkMemBlock memory;
    uint32_t offset;
    uint64_t size;
    Slot *slot;   // NULL: the buffer owns `memory`
    int pool, size_class;
} Buffer;

typedef struct {
    uint32_t type, format, flags, width, height, depth, mips, samples;
} DkpTextureDesc;

typedef struct {
    Context *ctx;
    DkMemBlock memory;
    DkImage image;
} Texture;

typedef struct {
    uint32_t type, format, ds_source, layer_offset, layer_count, mip_offset, mip_count;
    uint32_t swizzle[4];
    uint32_t descriptor; // bits: 1 sampled descriptor, 2 load/store descriptor
} DkpViewDesc;

typedef struct {
    Texture *texture;
    DkImageView view;
    int32_t slot, storage_slot; // image descriptor ids, or -1
} View;

typedef struct {
    uint32_t min_filter, mag_filter, mip_filter, wrap[3];
    float lod_min, lod_max, lod_bias;
    uint32_t compare_enable, compare_op;
    float border[4];
    float max_anisotropy;
} DkpSamplerDesc;

typedef struct {
    uint32_t buffer, offset, size, type, bgra, fixed;
} DkpAttrib;

typedef struct {
    uint32_t color_op, src_color, dst_color, alpha_op, src_alpha, dst_alpha;
} DkpBlend;

typedef struct {
    const uint8_t *vert;
    size_t vert_size;
    const uint8_t *frag;
    size_t frag_size;
    uint32_t primitive, restart_enable, restart_index;
    uint32_t cull_mode, front_face, polygon_mode, depth_clamp;
    uint32_t depth_bias_enable;
    float bias_constant, bias_slope, bias_clamp;
    uint32_t depth_test, depth_write, depth_compare;
    uint32_t stencil_test, stencil_front[4], stencil_back[4], stencil_read_mask, stencil_write_mask;
    uint32_t color_count, blend_enable_mask, write_masks;
    DkpBlend blend[DK_MAX_RENDER_TARGETS];
    uint32_t alpha_to_coverage, samples, sample_mask;
    uint32_t attrib_count;
    DkpAttrib attribs[DK_MAX_VERTEX_ATTRIBS];
    uint32_t buffer_count;
    uint32_t strides[DK_MAX_VERTEX_BUFFERS], divisors[DK_MAX_VERTEX_BUFFERS];
} DkpPipelineDesc;

typedef struct {
    Context *ctx;
    DkMemBlock code;
    DkShader shaders[2];
    DkPrimitive primitive;
    bool restart_enable;
    uint32_t restart_index;
    DkRasterizerState raster;
    bool depth_bias;
    float bias_constant, bias_slope, bias_clamp;
    DkMultisampleState multisample;
    uint32_t sample_mask;
    DkColorState color;
    DkColorWriteState writes;
    uint32_t color_count;
    DkBlendState blends[DK_MAX_RENDER_TARGETS];
    DkDepthStencilState depth_stencil;
    bool stencil;
    uint8_t stencil_read_mask, stencil_write_mask;
    uint32_t attrib_count, buffer_count;
    DkVtxAttribState attribs[DK_MAX_VERTEX_ATTRIBS];
    DkVtxBufferState buffers[DK_MAX_VERTEX_BUFFERS];
} Pipeline;

typedef struct {
    void *colors[DK_MAX_RENDER_TARGETS];
    uint32_t color_count;
    uint32_t clear_mask, discard_mask;
    uint32_t clear_values[DK_MAX_RENDER_TARGETS][4]; // raw bits; deko3d reads them per format
    void *depth;
    uint32_t depth_clear, stencil_clear, depth_discard;
    float depth_value;
    uint32_t stencil_value;
    uint32_t width, height;
    void *resolves[DK_MAX_RENDER_TARGETS]; // resolve target per color target, or NULL
} DkpPassDesc;

// One subresource region of a texture for copies: mip level, the first layer (or 3D slice) and extent.
typedef struct {
    uint32_t mip, x, y, z, width, height, depth;
} DkpRegion;

typedef struct {
    Context *ctx;
    DkCmdBuf buffer;
    // The recording is cut into deko3d command lists (segments) of about segment_entries gpfifo
    // entries each, so dkp_submit can kick them off in batches (see gpfifo_batch_entries).
    DkCmdList *lists;
    uint32_t *list_entries; // each segment's estimated gpfifo entries
    uint32_t list_count, list_capacity;
    uint32_t entries; // estimated gpfifo entries in the open segment
    CommandChunk *chunk;
    uint32_t remaining_words;
    bool failed, in_pass, started, finished;
    // A Full barrier waiting for the next command, with what it invalidates.
    bool barrier_pending;
    uint32_t barrier_invalidate;
    Pipeline *pipeline;
    uint32_t stencil_ref;
    uint32_t discard_mask;
    bool depth_discard;
    View *colors[DK_MAX_RENDER_TARGETS], *resolves[DK_MAX_RENDER_TARGETS];
    uint32_t colors_bound;
    // Upload arena (CPU-written, GPU-read) for per-draw data such as immediates.
    Slot *arena[16];
    uint32_t arena_count, arena_used;
} Command;

typedef struct {
    Context *ctx;
    DkMemBlock code;
    DkShader shader;
} ComputePipeline;

typedef struct {
    Context *ctx;
    DkSwapchain native;
} Swapchain;

// deko3d reports API misuse (debug library, libdeko3dd) and GPU errors (both libraries) here.
static void debug_message(void *user, const char *context, DkResult result, const char *message) {
    (void)user;
    FILE *log = fopen("sdmc:/switch/deko3d.log", "a");
    if (log) {
        fprintf(log, "%s: result %d: %s\n", context ? context : "?", (int)result, message ? message : "");
        fclose(log);
    }
}

// The release library (patches/deko3d/release-reports.patch) has no debug callback; it hands its
// fatal errors and a GPU fault's details (Queue::checkError) here before aborting.
void dkBhReport(DkResult result, const char *message) {
    debug_message(NULL, "deko3d", result, message);
}

static DkMemBlock make_memory(DkDevice device, uint64_t size, uint32_t flags) {
    const uint64_t mask = DK_MEMBLOCK_ALIGNMENT - 1;
    if (!size || size > UINT32_MAX - mask)
        return NULL;
    DkMemBlockMaker maker;
    dkMemBlockMakerDefaults(&maker, device, (uint32_t)((size + mask) & ~mask));
    maker.flags = flags;
    uint64_t started = armGetSystemTick();
    DkMemBlock block = dkMemBlockCreate(&maker);
    stat_end(stat_block_create, started);
    return block;
}

static void destroy_memory(DkMemBlock block) {
    uint64_t started = armGetSystemTick();
    dkMemBlockDestroy(block);
    stat_end(stat_block_destroy, started);
}

static void write_sampler(Context *ctx, uint32_t slot, const DkpSamplerDesc *desc) {
    DkSampler sampler;
    dkSamplerDefaults(&sampler);
    if (desc) {
        sampler.minFilter = desc->min_filter;
        sampler.magFilter = desc->mag_filter;
        sampler.mipFilter = desc->mip_filter;
        for (int i = 0; i < 3; ++i)
            sampler.wrapMode[i] = desc->wrap[i];
        sampler.lodClampMin = desc->lod_min;
        sampler.lodClampMax = desc->lod_max;
        sampler.lodBias = desc->lod_bias;
        sampler.compareEnable = desc->compare_enable;
        sampler.compareOp = desc->compare_op;
        for (int i = 0; i < 4; ++i)
            sampler.borderColor[i].value_f = desc->border[i];
        sampler.maxAnisotropy = desc->max_anisotropy;
    }
    dkSamplerDescriptorInitialize(&ctx->samplers[slot], &sampler);
}

void *dkp_open(void) {
    Context *ctx = calloc(1, sizeof(*ctx));
    if (!ctx)
        return NULL;
    mutexInit(&ctx->queue_lock);
    mutexInit(&ctx->alloc_lock);

    DkDeviceMaker device_maker;
    dkDeviceMakerDefaults(&device_maker);
    device_maker.flags = DkDeviceFlags_DepthZeroToOne | DkDeviceFlags_OriginUpperLeft;
    device_maker.cbDebug = debug_message;
    ctx->device = dkDeviceCreate(&device_maker);
    if (!ctx->device) {
        free(ctx);
        return NULL;
    }

    const uint32_t image_bytes = image_descriptor_count * sizeof(DkImageDescriptor);
    const uint32_t sampler_bytes = sampler_descriptor_count * sizeof(DkSamplerDescriptor);
    ctx->zeroes = make_memory(ctx->device, zero_block_size, mappable_flags);
    ctx->descriptors = make_memory(ctx->device, image_bytes + sampler_bytes, gpu_flags);
    if (ctx->zeroes && ctx->descriptors) {
        memset(dkMemBlockGetCpuAddr(ctx->zeroes), 0, zero_block_size);
        uint8_t *heap = dkMemBlockGetCpuAddr(ctx->descriptors);
        ctx->images = (DkImageDescriptor *)heap;
        ctx->samplers = (DkSamplerDescriptor *)(heap + image_bytes);
        // Stacks pop the lowest ids first. Sampler 0 stays a default sampler, for texelFetch.
        for (uint32_t i = 0; i < image_descriptor_count; ++i)
            ctx->free_images[i] = image_descriptor_count - 1 - i;
        ctx->image_top = image_descriptor_count;
        for (uint32_t i = 1; i < sampler_descriptor_count; ++i)
            ctx->free_samplers[i - 1] = sampler_descriptor_count - i;
        ctx->sampler_top = sampler_descriptor_count - 1;
        write_sampler(ctx, 0, NULL);

        DkQueueMaker queue_maker;
        dkQueueMakerDefaults(&queue_maker, ctx->device);
        queue_maker.flags = DkQueueFlags_Graphics | DkQueueFlags_Compute;
        ctx->queue = dkQueueCreate(&queue_maker);
        if (ctx->queue)
            return ctx;
    }
    if (ctx->descriptors)
        destroy_memory(ctx->descriptors);
    if (ctx->zeroes)
        destroy_memory(ctx->zeroes);
    dkDeviceDestroy(ctx->device);
    free(ctx);
    return NULL;
}

void dkp_close(void *ptr) {
    Context *ctx = ptr;
    if (!ctx)
        return;
    mutexLock(&ctx->queue_lock);
    if (ctx->queue) {
        // A healthy queue drains here; a failed queue is quiesced by destruction.
        dkQueueWaitIdle(ctx->queue);
        dkQueueDestroy(ctx->queue);
        ctx->queue = NULL;
    }
    while (ctx->orphaned_fences) {
        PendingFence *pending = ctx->orphaned_fences;
        ctx->orphaned_fences = pending->next;
        free(pending);
    }
    mutexUnlock(&ctx->queue_lock);
    while (ctx->free_chunks) {
        CommandChunk *chunk = ctx->free_chunks;
        ctx->free_chunks = chunk->previous;
        destroy_memory(chunk->memory);
        free(chunk);
    }
    while (ctx->slabs) {
        Slab *slab = ctx->slabs;
        ctx->slabs = slab->next;
        destroy_memory(slab->memory);
        free(slab->slots);
        free(slab);
    }
    destroy_memory(ctx->descriptors);
    destroy_memory(ctx->zeroes);
    dkDeviceDestroy(ctx->device);
    free(ctx);
}

// Buffers

static int size_class(uint64_t size) {
    if (size > (1u << slot_max_shift))
        return -1;
    int class = 0;
    while ((1ull << (slot_min_shift + class)) < size)
        ++class;
    return class;
}

static Slot *take_slot(Context *ctx, int pool, int class) {
    mutexLock(&ctx->alloc_lock);
    Slot *slot = ctx->free_slots[pool][class];
    if (slot)
        ctx->free_slots[pool][class] = slot->next;
    mutexUnlock(&ctx->alloc_lock);
    if (slot)
        return slot;
    const uint32_t slot_size = 1u << (slot_min_shift + class);
    const uint32_t block = slot_size > slab_size ? slot_size : slab_size;
    const uint32_t count = block / slot_size;
    Slab *slab = malloc(sizeof(*slab));
    Slot *slots = calloc(count, sizeof(*slots));
    DkMemBlock memory = slab && slots ? make_memory(ctx->device, block, pool ? mappable_flags : gpu_flags) : NULL;
    if (!memory) {
        free(slots);
        free(slab);
        return NULL;
    }
    slab->memory = memory;
    slab->slots = slots;
    for (uint32_t i = 0; i < count; ++i) {
        slots[i].memory = memory;
        slots[i].offset = i * slot_size;
        slots[i].next = i + 1 < count ? &slots[i + 1] : NULL;
    }
    mutexLock(&ctx->alloc_lock);
    slab->next = ctx->slabs;
    ctx->slabs = slab;
    slots[count - 1].next = ctx->free_slots[pool][class];
    ctx->free_slots[pool][class] = count > 1 ? &slots[1] : slots[count - 1].next;
    mutexUnlock(&ctx->alloc_lock);
    return &slots[0];
}

void *dkp_buffer(void *ptr, uint64_t size, bool mappable) {
    Context *ctx = ptr;
    if (!ctx)
        return NULL;
    Buffer *buffer = calloc(1, sizeof(*buffer));
    if (!buffer)
        return NULL;
    buffer->ctx = ctx;
    buffer->size = size;
    buffer->pool = mappable;
    buffer->size_class = size_class(size ? size : 1);
    if (buffer->size_class >= 0) {
        buffer->slot = take_slot(ctx, buffer->pool, buffer->size_class);
        if (buffer->slot) {
            buffer->memory = buffer->slot->memory;
            buffer->offset = buffer->slot->offset;
        }
    } else {
        buffer->memory = make_memory(ctx->device, size, mappable ? mappable_flags : gpu_flags);
    }
    if (!buffer->memory) {
        free(buffer);
        return NULL;
    }
    return buffer;
}

// wgpu-core frees a buffer only once no submitted work uses it, so its slot is reusable at once.
void dkp_buffer_free(void *ptr) {
    Buffer *buffer = ptr;
    if (!buffer)
        return;
    if (buffer->slot) {
        Context *ctx = buffer->ctx;
        mutexLock(&ctx->alloc_lock);
        buffer->slot->next = ctx->free_slots[buffer->pool][buffer->size_class];
        ctx->free_slots[buffer->pool][buffer->size_class] = buffer->slot;
        mutexUnlock(&ctx->alloc_lock);
    } else {
        destroy_memory(buffer->memory);
    }
    free(buffer);
}

void *dkp_buffer_data(void *ptr) {
    Buffer *buffer = ptr;
    return buffer ? (uint8_t *)dkMemBlockGetCpuAddr(buffer->memory) + buffer->offset : NULL;
}

uint64_t dkp_buffer_address(void *ptr) {
    Buffer *buffer = ptr;
    return buffer ? dkMemBlockGetGpuAddr(buffer->memory) + buffer->offset : DK_GPU_ADDR_INVALID;
}

// Textures and views

void *dkp_texture(void *ptr, const DkpTextureDesc *desc) {
    Context *ctx = ptr;
    if (!ctx || !desc)
        return NULL;
    Texture *texture = malloc(sizeof(*texture));
    if (!texture)
        return NULL;
    DkImageLayoutMaker maker;
    DkImageLayout layout;
    dkImageLayoutMakerDefaults(&maker, ctx->device);
    maker.type = desc->type;
    maker.flags = desc->flags;
    maker.format = desc->format;
    maker.msMode = desc->samples == 8 ? DkMsMode_8x : desc->samples == 4 ? DkMsMode_4x
                 : desc->samples == 2 ? DkMsMode_2x : DkMsMode_1x;
    maker.dimensions[0] = desc->width;
    maker.dimensions[1] = desc->height;
    maker.dimensions[2] = desc->depth;
    maker.mipLevels = desc->mips;
    dkImageLayoutInitialize(&layout, &maker);
    texture->memory = make_memory(ctx->device, dkImageLayoutGetSize(&layout), DkMemBlockFlags_Image | gpu_flags);
    if (!texture->memory) {
        free(texture);
        return NULL;
    }
    texture->ctx = ctx;
    dkImageInitialize(&texture->image, &layout, texture->memory, 0);
    return texture;
}

void dkp_texture_free(void *ptr) {
    Texture *texture = ptr;
    if (!texture)
        return;
    destroy_memory(texture->memory);
    free(texture);
}

void dkp_view_free(void *ptr);

void *dkp_view(void *texture_ptr, const DkpViewDesc *desc) {
    Texture *texture = texture_ptr;
    if (!texture || !desc)
        return NULL;
    View *view = malloc(sizeof(*view));
    if (!view)
        return NULL;
    view->texture = texture;
    view->slot = view->storage_slot = -1;
    dkImageViewDefaults(&view->view, &texture->image);
    view->view.type = desc->type;
    view->view.format = desc->format;
    for (int i = 0; i < 4; ++i)
        view->view.swizzle[i] = desc->swizzle[i];
    view->view.dsSource = desc->ds_source;
    view->view.layerOffset = desc->layer_offset;
    view->view.layerCount = desc->layer_count;
    view->view.mipLevelOffset = desc->mip_offset;
    view->view.mipLevelCount = desc->mip_count;
    if (desc->descriptor) {
        Context *ctx = texture->ctx;
        // Sampling and load/store need differently initialized descriptors.
        mutexLock(&ctx->alloc_lock);
        if ((desc->descriptor & 1) && ctx->image_top)
            view->slot = (int32_t)ctx->free_images[--ctx->image_top];
        if ((desc->descriptor & 2) && ctx->image_top)
            view->storage_slot = (int32_t)ctx->free_images[--ctx->image_top];
        mutexUnlock(&ctx->alloc_lock);
        if (((desc->descriptor & 1) && view->slot < 0) || ((desc->descriptor & 2) && view->storage_slot < 0)) {
            dkp_view_free(view);
            return NULL;
        }
        // Command lists invalidate the descriptor cache before use (see begin()).
        if (view->slot >= 0)
            dkImageDescriptorInitialize(&ctx->images[view->slot], &view->view, false, false);
        if (view->storage_slot >= 0)
            dkImageDescriptorInitialize(&ctx->images[view->storage_slot], &view->view, true, false);
    }
    return view;
}

void dkp_view_slots(void *ptr, int32_t *sampled, int32_t *storage) {
    View *view = ptr;
    *sampled = view ? view->slot : -1;
    *storage = view ? view->storage_slot : -1;
}

void dkp_view_free(void *ptr) {
    View *view = ptr;
    if (!view)
        return;
    Context *ctx = view->texture->ctx;
    mutexLock(&ctx->alloc_lock);
    if (view->slot >= 0)
        ctx->free_images[ctx->image_top++] = (uint32_t)view->slot;
    if (view->storage_slot >= 0)
        ctx->free_images[ctx->image_top++] = (uint32_t)view->storage_slot;
    mutexUnlock(&ctx->alloc_lock);
    free(view);
}

int32_t dkp_sampler(void *ptr, const DkpSamplerDesc *desc) {
    Context *ctx = ptr;
    if (!ctx || !desc)
        return -1;
    int32_t slot = -1;
    mutexLock(&ctx->alloc_lock);
    if (ctx->sampler_top)
        slot = (int32_t)ctx->free_samplers[--ctx->sampler_top];
    mutexUnlock(&ctx->alloc_lock);
    if (slot >= 0)
        write_sampler(ctx, (uint32_t)slot, desc);
    return slot;
}

void dkp_sampler_free(void *ptr, int32_t slot) {
    Context *ctx = ptr;
    if (!ctx || slot <= 0)
        return;
    mutexLock(&ctx->alloc_lock);
    ctx->free_samplers[ctx->sampler_top++] = (uint32_t)slot;
    mutexUnlock(&ctx->alloc_lock);
}

// Pipelines

static uint32_t read_le32(const uint8_t *bytes) {
    return (uint32_t)bytes[0] | (uint32_t)bytes[1] << 8 | (uint32_t)bytes[2] << 16 |
           (uint32_t)bytes[3] << 24;
}

// Single-program DKSH images from UAM. Check section and program bounds before deko3d
// reads them (its debug checks are compiled out of the release library).
static bool check_dksh(const uint8_t *bytes, size_t size, uint32_t type) {
    enum { header_size = 24, program_size = 64 };
    const uint32_t mask = DK_SHADER_CODE_ALIGNMENT - 1;
    if (!bytes || size < header_size || size > UINT32_MAX || memcmp(bytes, "DKSH", 4))
        return false;
    const uint32_t control = read_le32(bytes + 8);
    const uint32_t code = read_le32(bytes + 12);
    const uint32_t program = read_le32(bytes + 16);
    if (read_le32(bytes + 4) != header_size || read_le32(bytes + 20) != 1 ||
        program < header_size || control < program || control - program < program_size ||
        (control & mask) || !code || (code & mask) || (uint64_t)control + code != size)
        return false;
    const uint8_t *entry = bytes + program;
    const uint32_t constants = read_le32(entry + 12);
    return read_le32(entry) == type && read_le32(entry + 4) < code && constants <= code &&
           read_le32(entry + 16) <= code - constants &&
           (type != 0 || !read_le32(entry + 28) || read_le32(entry + 24) < code);
}

void *dkp_pipeline(void *ptr, const DkpPipelineDesc *desc) {
    Context *ctx = ptr;
    if (!ctx || !desc || !check_dksh(desc->vert, desc->vert_size, 0) ||
        !check_dksh(desc->frag, desc->frag_size, 1) || desc->color_count > DK_MAX_RENDER_TARGETS ||
        desc->attrib_count > DK_MAX_VERTEX_ATTRIBS || desc->buffer_count > DK_MAX_VERTEX_BUFFERS)
        return NULL;
    Pipeline *p = calloc(1, sizeof(*p));
    if (!p)
        return NULL;
    p->ctx = ctx;
    // DKSH sizes are multiples of DK_SHADER_CODE_ALIGNMENT, so the fragment program stays aligned.
    p->code = make_memory(ctx->device, (uint64_t)desc->vert_size + desc->frag_size + DK_SHADER_CODE_UNUSABLE_SIZE,
                          gpu_flags | DkMemBlockFlags_Code);
    if (!p->code) {
        free(p);
        return NULL;
    }
    uint8_t *code = dkMemBlockGetCpuAddr(p->code);
    memcpy(code, desc->vert, desc->vert_size);
    memcpy(code + desc->vert_size, desc->frag, desc->frag_size);
    DkShaderMaker maker;
    dkShaderMakerDefaults(&maker, p->code, 0);
    dkShaderInitialize(&p->shaders[0], &maker);
    dkShaderMakerDefaults(&maker, p->code, (uint32_t)desc->vert_size);
    dkShaderInitialize(&p->shaders[1], &maker);
    if (!dkShaderIsValid(&p->shaders[0]) || dkShaderGetStage(&p->shaders[0]) != DkStage_Vertex ||
        !dkShaderIsValid(&p->shaders[1]) || dkShaderGetStage(&p->shaders[1]) != DkStage_Fragment) {
        destroy_memory(p->code);
        free(p);
        return NULL;
    }

    p->primitive = desc->primitive;
    p->restart_enable = desc->restart_enable;
    p->restart_index = desc->restart_index;
    dkRasterizerStateDefaults(&p->raster);
    p->raster.cullMode = desc->cull_mode;
    p->raster.frontFace = desc->front_face;
    p->raster.polygonModeFront = desc->polygon_mode;
    p->raster.polygonModeBack = desc->polygon_mode;
    p->raster.depthClampEnable = desc->depth_clamp;
    // WebGPU's flat interpolation takes the first vertex.
    p->raster.provokingVertex = DkProvokingVertex_First;
    p->raster.depthBiasEnableMask = desc->depth_bias_enable ? DkPolygonFlag_All : 0;
    p->depth_bias = desc->depth_bias_enable;
    p->bias_constant = desc->bias_constant;
    p->bias_slope = desc->bias_slope;
    p->bias_clamp = desc->bias_clamp;
    dkMultisampleStateDefaults(&p->multisample);
    const DkMsMode ms = desc->samples == 8 ? DkMsMode_8x : desc->samples == 4 ? DkMsMode_4x
                      : desc->samples == 2 ? DkMsMode_2x : DkMsMode_1x;
    p->multisample.mode = ms;
    p->multisample.rasterizerMode = ms;
    p->multisample.alphaToCoverageEnable = desc->alpha_to_coverage;
    // The defaults put every sample at the pixel centre; this loads the standard pattern.
    dkMultisampleStateSetLocations(&p->multisample, NULL, 0);
    // The hardware's sample mask registers are 16 bits; wider values fault (as Mesa's nvc0 masks).
    p->sample_mask = desc->sample_mask & 0xffff;

    dkColorStateDefaults(&p->color);
    p->color.blendEnableMask = desc->blend_enable_mask;
    p->writes.masks = desc->write_masks;
    p->color_count = desc->color_count;
    for (uint32_t i = 0; i < desc->color_count; ++i) {
        // The hardware rejects a zero blend equation even for targets with blending off, so
        // those keep deko3d's defaults. The alpha destination factor needs patches/deko3d:
        // devkitPro's deko3d 0.5.0 writes it from dstColorBlendFactor.
        dkBlendStateDefaults(&p->blends[i]);
        if (desc->blend_enable_mask >> i & 1) {
            const DkpBlend *b = &desc->blend[i];
            dkBlendStateSetOps(&p->blends[i], b->color_op, b->alpha_op);
            dkBlendStateSetFactors(&p->blends[i], b->src_color, b->dst_color, b->src_alpha, b->dst_alpha);
        }
    }

    dkDepthStencilStateDefaults(&p->depth_stencil);
    p->depth_stencil.depthTestEnable = desc->depth_test;
    p->depth_stencil.depthWriteEnable = desc->depth_write;
    p->depth_stencil.depthCompareOp = desc->depth_compare;
    p->depth_stencil.stencilTestEnable = desc->stencil_test;
    p->depth_stencil.stencilFrontFailOp = desc->stencil_front[0];
    p->depth_stencil.stencilFrontPassOp = desc->stencil_front[1];
    p->depth_stencil.stencilFrontDepthFailOp = desc->stencil_front[2];
    p->depth_stencil.stencilFrontCompareOp = desc->stencil_front[3];
    p->depth_stencil.stencilBackFailOp = desc->stencil_back[0];
    p->depth_stencil.stencilBackPassOp = desc->stencil_back[1];
    p->depth_stencil.stencilBackDepthFailOp = desc->stencil_back[2];
    p->depth_stencil.stencilBackCompareOp = desc->stencil_back[3];
    p->stencil = desc->stencil_test;
    p->stencil_read_mask = (uint8_t)desc->stencil_read_mask;
    p->stencil_write_mask = (uint8_t)desc->stencil_write_mask;

    p->attrib_count = desc->attrib_count;
    for (uint32_t i = 0; i < desc->attrib_count; ++i) {
        const DkpAttrib *a = &desc->attribs[i];
        DkVtxAttribState *s = &p->attribs[i];
        memset(s, 0, sizeof(*s));
        s->bufferId = a->buffer;
        s->isFixed = a->fixed;
        s->offset = a->offset;
        s->size = a->size;
        s->type = a->type;
        s->isBgra = a->bgra;
    }
    p->buffer_count = desc->buffer_count;
    for (uint32_t i = 0; i < desc->buffer_count; ++i) {
        p->buffers[i].stride = desc->strides[i];
        p->buffers[i].divisor = desc->divisors[i];
    }
    return p;
}

void dkp_pipeline_free(void *ptr) {
    Pipeline *pipeline = ptr;
    if (!pipeline)
        return;
    destroy_memory(pipeline->code);
    free(pipeline);
}

void *dkp_compute_pipeline(void *ptr, const uint8_t *dksh, size_t size) {
    enum { compute_type = 5 }; // DkshProgramType_Compute
    Context *ctx = ptr;
    if (!ctx || !check_dksh(dksh, size, compute_type))
        return NULL;
    ComputePipeline *p = calloc(1, sizeof(*p));
    if (!p)
        return NULL;
    p->ctx = ctx;
    p->code = make_memory(ctx->device, (uint64_t)size + DK_SHADER_CODE_UNUSABLE_SIZE, gpu_flags | DkMemBlockFlags_Code);
    if (!p->code) {
        free(p);
        return NULL;
    }
    memcpy(dkMemBlockGetCpuAddr(p->code), dksh, size);
    DkShaderMaker maker;
    dkShaderMakerDefaults(&maker, p->code, 0);
    dkShaderInitialize(&p->shader, &maker);
    if (!dkShaderIsValid(&p->shader) || dkShaderGetStage(&p->shader) != DkStage_Compute) {
        destroy_memory(p->code);
        free(p);
        return NULL;
    }
    return p;
}

void dkp_compute_pipeline_free(void *ptr) {
    ComputePipeline *p = ptr;
    if (!p)
        return;
    destroy_memory(p->code);
    free(p);
}

// Command recording

static bool require(Command *cmd, bool ok) {
    if (!cmd || cmd->failed || cmd->finished)
        return false;
    if (!ok)
        cmd->failed = true;
    return ok;
}

// Pops a retired chunk, or makes one. Callers' fences guarantee the GPU is done with pooled chunks.
static CommandChunk *take_chunk(Context *ctx) {
    mutexLock(&ctx->alloc_lock);
    CommandChunk *chunk = ctx->free_chunks;
    if (chunk) {
        ctx->free_chunks = chunk->previous;
        --ctx->free_count;
    }
    mutexUnlock(&ctx->alloc_lock);
    if (chunk)
        return chunk;
    chunk = malloc(sizeof(*chunk));
    if (!chunk)
        return NULL;
    chunk->memory = make_memory(ctx->device, command_chunk_size, gpu_flags);
    if (!chunk->memory) {
        free(chunk);
        return NULL;
    }
    return chunk;
}

// Reserve conservative upper bounds for these fixed-size deko3d commands. Grow before
// recording, since deko3d treats failure inside its add-memory callback as fatal.
static bool reserve(Command *cmd, uint32_t words) {
    if (words > cmd->remaining_words) {
        CommandChunk *chunk = take_chunk(cmd->ctx);
        if (!chunk)
            return require(cmd, false);
        chunk->previous = cmd->chunk;
        cmd->chunk = chunk;
        dkCmdBufAddMemory(cmd->buffer, chunk->memory, 0, command_chunk_size);
        cmd->remaining_words = command_chunk_size / sizeof(uint32_t) - 64;
        cmd->entries += 1; // ends the entry the last chunk was filling
    }
    cmd->remaining_words -= words;
    return true;
}

// Ends the open segment: what was recorded since becomes one deko3d command list.
static bool end_segment(Command *cmd) {
    if (cmd->list_count == cmd->list_capacity) {
        uint32_t capacity = cmd->list_capacity ? cmd->list_capacity * 2 : 4;
        DkCmdList *lists = realloc(cmd->lists, capacity * sizeof(*lists));
        if (lists)
            cmd->lists = lists;
        uint32_t *entries = lists ? realloc(cmd->list_entries, capacity * sizeof(*entries)) : NULL;
        if (!require(cmd, entries != NULL))
            return false;
        cmd->list_entries = entries;
        cmd->list_capacity = capacity;
    }
    DkCmdList list = dkCmdBufFinishList(cmd->buffer);
    if (list) {
        cmd->lists[cmd->list_count] = list;
        cmd->list_entries[cmd->list_count] = cmd->entries + 1; // and the entry FinishList ends
        ++cmd->list_count;
    }
    cmd->entries = 0;
    return true;
}

// Every recording entry point starts here. The first command in a list binds the descriptor
// heaps and invalidates the descriptor cache: descriptors may have been rewritten since the
// last list ran (only for slots no submitted work still uses). The GPU runs a recording's
// segments back to back, so its state carries across them.
static bool begin(Command *cmd, uint32_t words) {
    if (!require(cmd, true))
        return false;
    if (cmd->entries >= segment_entries && !end_segment(cmd))
        return false;
    if (!cmd->started) {
        if (!reserve(cmd, 32))
            return false;
        Context *ctx = cmd->ctx;
        const DkGpuAddr heap = dkMemBlockGetGpuAddr(ctx->descriptors);
        dkCmdBufBindImageDescriptorSet(cmd->buffer, heap, image_descriptor_count);
        dkCmdBufBindSamplerDescriptorSet(cmd->buffer, heap + image_descriptor_count * sizeof(DkImageDescriptor),
                                         sampler_descriptor_count);
        // Also drop constant-cache lines: arena and buffer slots are reused across lists.
        dkCmdBufBarrier(cmd->buffer, DkBarrier_None, DkInvalidateFlags_Descriptors | DkInvalidateFlags_Shader);
        cmd->started = true;
    }
    if (cmd->barrier_pending) {
        if (!reserve(cmd, 16))
            return false;
        dkCmdBufBarrier(cmd->buffer, DkBarrier_Full, cmd->barrier_invalidate);
        cmd->entries += 2; // a Full barrier ends two entries (maxwell/gpu_base.cpp)
        cmd->barrier_pending = false;
        cmd->barrier_invalidate = 0;
    }
    return reserve(cmd, words);
}

void *dkp_cmd(void *ptr) {
    Context *ctx = ptr;
    if (!ctx)
        return NULL;
    Command *cmd = calloc(1, sizeof(*cmd));
    if (!cmd)
        return NULL;
    cmd->ctx = ctx;
    DkCmdBufMaker maker;
    dkCmdBufMakerDefaults(&maker, ctx->device);
    uint64_t started = armGetSystemTick();
    cmd->buffer = dkCmdBufCreate(&maker);
    stat_end(stat_cmdbuf_create, started);
    if (!cmd->buffer) {
        free(cmd);
        return NULL;
    }
    return cmd;
}

// Only called once the GPU has finished with the command list, or it was never submitted.
void dkp_cmd_free(void *ptr) {
    Command *cmd = ptr;
    if (!cmd)
        return;
    uint64_t started = armGetSystemTick();
    Context *ctx = cmd->ctx;
    dkCmdBufDestroy(cmd->buffer);
    for (uint32_t i = 0; i < cmd->arena_count; ++i) {
        mutexLock(&ctx->alloc_lock);
        cmd->arena[i]->next = ctx->free_slots[1][arena_class];
        ctx->free_slots[1][arena_class] = cmd->arena[i];
        mutexUnlock(&ctx->alloc_lock);
    }
    while (cmd->chunk) {
        CommandChunk *chunk = cmd->chunk;
        cmd->chunk = chunk->previous;
        mutexLock(&ctx->alloc_lock);
        bool pooled = ctx->free_count < chunk_pool_limit;
        if (pooled) {
            chunk->previous = ctx->free_chunks;
            ctx->free_chunks = chunk;
            ++ctx->free_count;
        }
        mutexUnlock(&ctx->alloc_lock);
        if (!pooled) {
            destroy_memory(chunk->memory);
            free(chunk);
        }
    }
    free(cmd->lists);
    free(cmd->list_entries);
    free(cmd);
    stat_end(stat_cmd_free, started);
}

static bool finish(Command *cmd) {
    if (cmd->failed || cmd->in_pass)
        return false;
    if (!cmd->finished && (cmd->started || cmd->barrier_pending)) {
        // Complete copy-engine operations before the graphics queue's fence: begin records the
        // Full barrier, merged with one the recording ended on.
        cmd->barrier_pending = true;
        if (!begin(cmd, 0) || !end_segment(cmd))
            return false;
    }
    cmd->finished = true;
    return true;
}

// Queue and fences

static void lose_queue_locked(Context *ctx) {
    ctx->queue_lost = true;
    // Core may discard submitted command buffers after a DeviceError. The
    // native queue must stop touching their memory before the error escapes.
    if (ctx->queue) {
        dkQueueDestroy(ctx->queue);
        ctx->queue = NULL;
    }
}

static bool queue_healthy_locked(Context *ctx) {
    if (!ctx->queue || ctx->queue_lost)
        return false;
    if (dkQueueIsInErrorState(ctx->queue)) {
        lose_queue_locked(ctx);
        return false;
    }
    return true;
}

static void retire_head_locked(Fence *fence) {
    PendingFence *pending = fence->head;
    fence->head = pending->next;
    if (!fence->head)
        fence->tail = NULL;
    fence->completed = pending->value;
    free(pending);
}

void *dkp_fence_create(void *ptr) {
    Context *ctx = ptr;
    if (!ctx)
        return NULL;
    Fence *fence = calloc(1, sizeof(*fence));
    if (fence)
        fence->ctx = ctx;
    return fence;
}

void dkp_fence_destroy(void *ptr) {
    Fence *fence = ptr;
    if (!fence)
        return;
    Context *ctx = fence->ctx;
    mutexLock(&ctx->queue_lock);
    while (fence->head && queue_healthy_locked(ctx)) {
        DkResult result = dkFenceWait(&fence->head->native, -1);
        if (result != DkResult_Success) {
            lose_queue_locked(ctx);
            break;
        }
        retire_head_locked(fence);
    }
    if (fence->head) {
        // A failed queue may not have signaled every fence. Keep their storage
        // alive until the queue itself is destroyed rather than waiting forever.
        fence->tail->next = ctx->orphaned_fences;
        ctx->orphaned_fences = fence->head;
    }
    mutexUnlock(&ctx->queue_lock);
    free(fence);
}

bool dkp_fence_completed(void *ptr, uint64_t *completed) {
    Fence *fence = ptr;
    if (!fence || !completed)
        return false;
    Context *ctx = fence->ctx;
    uint64_t started = armGetSystemTick();
    lock_queue(ctx);
    bool ok = queue_healthy_locked(ctx);
    bool polled = false;
    while (ok && fence->head) {
        polled = true;
        DkResult result = dkFenceWait(&fence->head->native, 0);
        if (result == DkResult_Success)
            retire_head_locked(fence);
        else if (result == DkResult_Timeout)
            break;
        else {
            lose_queue_locked(ctx);
            ok = false;
        }
    }
    if (ok && polled)
        ok = queue_healthy_locked(ctx);
    if (ok)
        *completed = fence->completed;
    mutexUnlock(&ctx->queue_lock);
    stat_end(stat_fence_poll, started);
    return ok;
}

int dkp_fence_wait(void *ptr, uint64_t value, int64_t timeout_ns) {
    Fence *fence = ptr;
    if (!fence || timeout_ns < -1)
        return -1;
    uint64_t started = armGetSystemTick();
    Context *ctx = fence->ctx;
    lock_queue(ctx);
    int result = 1;
    if (value > fence->submitted || (value > fence->completed && !queue_healthy_locked(ctx))) {
        lose_queue_locked(ctx);
        result = -1;
        goto done;
    }
    while (fence->completed < value) {
        if (!fence->head) {
            lose_queue_locked(ctx);
            result = -1;
            break;
        }
        int64_t remaining = -1;
        if (timeout_ns >= 0) {
            uint64_t elapsed = armTicksToNs(armGetSystemTick() - started);
            remaining = elapsed >= (uint64_t)timeout_ns ? 0 : timeout_ns - (int64_t)elapsed;
        }
        DkResult waited = dkFenceWait(&fence->head->native, remaining);
        if (waited == DkResult_Success) {
            retire_head_locked(fence);
            if (!queue_healthy_locked(ctx)) {
                result = -1;
                break;
            }
        } else if (waited == DkResult_Timeout) {
            result = queue_healthy_locked(ctx) ? 0 : -1;
            break;
        } else {
            lose_queue_locked(ctx);
            result = -1;
            break;
        }
    }
done:
    mutexUnlock(&ctx->queue_lock);
    stat_end(stat_fence_wait, started);
    return result;
}

// Forgets kickoffs the GPU has finished, then waits for the oldest ones until `entries` more fit
// in ring_budget and a kick slot is free.
static bool make_room_locked(Context *ctx, uint32_t entries) {
    while (ctx->kick_count) {
        const bool full = ctx->kick_count == kick_slots || ctx->kick_entries + entries > ring_budget;
        const uint32_t oldest = ctx->kick_head;
        const uint64_t started = armGetSystemTick();
        DkResult waited = dkFenceWait(&ctx->kicks[oldest].fence, full ? 2000000000 : 0);
        if (waited == DkResult_Timeout && full) {
            char message[160];
            snprintf(message, sizeof(message), "the GPU has not finished a kickoff in 2 s; still waiting "
                     "(%u entries in %u kickoffs outstanding)", (unsigned)ctx->kick_entries, (unsigned)ctx->kick_count);
            debug_message(NULL, "dkp_submit", DkResult_Timeout, message);
            waited = dkFenceWait(&ctx->kicks[oldest].fence, -1);
        }
        if (waited == DkResult_Timeout)
            break;
        if (waited != DkResult_Success || !queue_healthy_locked(ctx))
            return false;
        const uint64_t waited_ticks = armGetSystemTick() - started;
        if (full)
            stat_add(stat_ring_wait, waited_ticks);
        const uint64_t ms = armTicksToNs(waited_ticks) / 1000000;
        if (ms >= 500) {
            char message[160];
            snprintf(message, sizeof(message), "waited %u ms for the GPU to make room on the gpfifo ring "
                     "(%u entries in %u kickoffs outstanding)", (unsigned)ms, (unsigned)ctx->kick_entries,
                     (unsigned)ctx->kick_count);
            debug_message(NULL, "dkp_submit", DkResult_Success, message);
        }
        ctx->kick_entries -= ctx->kicks[oldest].entries;
        ctx->kick_head = (oldest + 1) % kick_slots;
        --ctx->kick_count;
    }
    return true;
}

// Kicks off what the queue holds, counted as `entries` gpfifo entries, with a fence that says
// when the GPU is past them. make_room_locked has left a slot free.
static bool kick_locked(Context *ctx, uint32_t entries) {
    const uint32_t slot = (ctx->kick_head + ctx->kick_count) % kick_slots;
    const uint64_t started = armGetSystemTick();
    dkQueueSignalFence(ctx->queue, &ctx->kicks[slot].fence, false);
    if (!queue_healthy_locked(ctx))
        return false;
    dkQueueFlush(ctx->queue);
    stat_end(stat_kick, started);
    if (!queue_healthy_locked(ctx))
        return false;
    // nvservices adds up to two entries of its own to each kickoff (its fence wait and increment).
    ctx->kicks[slot].entries = entries + 2;
    ++ctx->kick_count;
    ctx->kick_entries += entries + 2;
    return true;
}

bool dkp_submit(void *ptr, const void *const *commands, size_t count, void *fence_ptr, uint64_t value) {
    Context *ctx = ptr;
    Fence *fence = fence_ptr;
    if (!ctx || !fence || fence->ctx != ctx || (count && !commands))
        return false;

    uint64_t started = armGetSystemTick();
    lock_queue(ctx);
    bool ok = false;
    PendingFence *pending = NULL;
    bool attached = false;
    if (!queue_healthy_locked(ctx) || value <= fence->submitted)
        goto done;
    for (size_t i = 0; i < count; ++i) {
        Command *cmd = (Command *)commands[i];
        if (!cmd || cmd->ctx != ctx || !finish(cmd))
            goto done;
    }
    // Allocate before submitting any GPU work; the queue writes into this
    // address asynchronously and it must remain stable until completion.
    pending = malloc(sizeof(*pending));
    if (!pending)
        goto done;
    pending->next = NULL;
    pending->value = value;
    // Segments go to the queue in batches of at most gpfifo_batch_entries gpfifo entries, each
    // started once the ring has room for a whole batch.
    if (!make_room_locked(ctx, gpfifo_batch_entries))
        goto done;
    uint32_t batched = 0, submitted_entries = 0;
    for (size_t i = 0; i < count; ++i) {
        const Command *cmd = commands[i];
        for (uint32_t j = 0; j < cmd->list_count; ++j) {
            // The queue adds an entry or two of its own per list.
            const uint32_t entries = cmd->list_entries[j] + 2;
            if (batched && batched + entries > gpfifo_batch_entries) {
                if (!kick_locked(ctx, batched) || !make_room_locked(ctx, gpfifo_batch_entries))
                    goto done;
                batched = 0;
            }
            const uint64_t queued = armGetSystemTick();
            dkQueueSubmitCommands(ctx->queue, cmd->lists[j]);
            stat_end(stat_queue_commands, queued);
            if (!queue_healthy_locked(ctx))
                goto done;
            batched += entries;
            submitted_entries += entries;
        }
    }
    if (fence->tail)
        fence->tail->next = pending;
    else
        fence->head = pending;
    fence->tail = pending;
    attached = true;
    dkQueueSignalFence(ctx->queue, &pending->native, true);
    if (!queue_healthy_locked(ctx))
        goto done;
    // The fences add a few entries.
    if (!kick_locked(ctx, batched + 4))
        goto done;
    fence->submitted = value;
    stat_add(stat_entries, submitted_entries);
    ok = true;
done:
    if (!attached)
        free(pending);
    if (!ok)
        lose_queue_locked(ctx);
    mutexUnlock(&ctx->queue_lock);
    stat_end(stat_submit, started);
    return ok;
}

// Blocks until the queue has run everything submitted to it, presents included.
bool dkp_wait_idle(void *ptr) {
    Context *ctx = ptr;
    if (!ctx)
        return false;
    const uint64_t started = armGetSystemTick();
    lock_queue(ctx);
    bool ok = queue_healthy_locked(ctx);
    if (ok) {
        dkQueueWaitIdle(ctx->queue);
        ok = queue_healthy_locked(ctx);
    }
    mutexUnlock(&ctx->queue_lock);
    stat_end(stat_wait_idle, started);
    return ok;
}

// Render passes and state

void dkp_pass(void *command, const DkpPassDesc *desc) {
    Command *cmd = command;
    if (!desc || !require(cmd, !cmd->in_pass && desc->color_count <= DK_MAX_RENDER_TARGETS &&
                          desc->width && desc->height) || !begin(cmd, 160))
        return;
    const DkImageView *targets[DK_MAX_RENDER_TARGETS] = {0};
    for (uint32_t i = 0; i < desc->color_count; ++i) {
        View *view = desc->colors[i];
        targets[i] = view ? &view->view : NULL;
    }
    const View *depth = desc->depth;
    dkCmdBufBindRenderTargets(cmd->buffer, targets, desc->color_count, depth ? &depth->view : NULL);
    const DkViewport viewport = {0, 0, (float)desc->width, (float)desc->height, 0, 1};
    const DkScissor scissor = {0, 0, desc->width, desc->height};
    dkCmdBufSetViewports(cmd->buffer, 0, &viewport, 1);
    dkCmdBufSetScissors(cmd->buffer, 0, &scissor, 1);
    for (uint32_t i = 0; i < desc->color_count; ++i)
        if (targets[i] && (desc->clear_mask >> i & 1))
            dkCmdBufClearColor(cmd->buffer, i, DkColorMask_RGBA, desc->clear_values[i]);
    if (depth && (desc->depth_clear || desc->stencil_clear))
        dkCmdBufClearDepthStencil(cmd->buffer, desc->depth_clear, desc->depth_value,
                                  desc->stencil_clear ? 0xff : 0, (uint8_t)desc->stencil_value);
    cmd->in_pass = true;
    cmd->pipeline = NULL;
    cmd->discard_mask = desc->discard_mask;
    cmd->depth_discard = depth && desc->depth_discard;
    cmd->colors_bound = desc->color_count;
    for (uint32_t i = 0; i < DK_MAX_RENDER_TARGETS; ++i) {
        cmd->colors[i] = i < desc->color_count ? desc->colors[i] : NULL;
        cmd->resolves[i] = i < desc->color_count ? desc->resolves[i] : NULL;
    }
}

void dkp_end_pass(void *command) {
    Command *cmd = command;
    if (!require(cmd, cmd->in_pass) || !begin(cmd, 48 + DK_MAX_RENDER_TARGETS * 64))
        return;
    bool resolving = false;
    for (uint32_t i = 0; i < cmd->colors_bound; ++i) {
        if (!cmd->resolves[i])
            continue;
        if (!resolving) {
            // The 2D engine reads the multisampled targets once rendering has finished.
            dkCmdBufBarrier(cmd->buffer, DkBarrier_Fragments, 0);
            resolving = true;
        }
        dkCmdBufResolveImage(cmd->buffer, &cmd->colors[i]->view, &cmd->resolves[i]->view);
    }
    for (uint32_t i = 0; i < DK_MAX_RENDER_TARGETS; ++i)
        if (cmd->discard_mask >> i & 1)
            dkCmdBufDiscardColor(cmd->buffer, i);
    if (cmd->depth_discard)
        dkCmdBufDiscardDepthStencil(cmd->buffer);
    cmd->in_pass = false;
}

static void apply_stencil(Command *cmd) {
    const Pipeline *p = cmd->pipeline;
    if (p && p->stencil)
        dkCmdBufSetStencil(cmd->buffer, DkFace_FrontAndBack, p->stencil_write_mask, (uint8_t)cmd->stencil_ref,
                           p->stencil_read_mask);
}

void dkp_bind(void *command, void *pipeline_ptr) {
    Command *cmd = command;
    Pipeline *p = pipeline_ptr;
    if (!require(cmd, cmd->in_pass && p && p->ctx == cmd->ctx) || !begin(cmd, 640))
        return;
    const DkShader *shaders[] = {&p->shaders[0], &p->shaders[1]};
    dkCmdBufBindShaders(cmd->buffer, DkStageFlag_GraphicsMask, shaders, 2);
    dkCmdBufBindRasterizerState(cmd->buffer, &p->raster);
    dkCmdBufBindMultisampleState(cmd->buffer, &p->multisample);
    dkCmdBufSetSampleMask(cmd->buffer, p->sample_mask);
    dkCmdBufBindColorState(cmd->buffer, &p->color);
    dkCmdBufBindColorWriteState(cmd->buffer, &p->writes);
    if (p->color_count)
        dkCmdBufBindBlendStates(cmd->buffer, 0, p->blends, p->color_count);
    dkCmdBufBindDepthStencilState(cmd->buffer, &p->depth_stencil);
    dkCmdBufBindVtxAttribState(cmd->buffer, p->attribs, p->attrib_count);
    dkCmdBufBindVtxBufferState(cmd->buffer, p->buffers, p->buffer_count);
    dkCmdBufSetPrimitiveRestart(cmd->buffer, p->restart_enable, p->restart_index);
    if (p->depth_bias)
        dkCmdBufSetDepthBias(cmd->buffer, p->bias_constant, p->bias_clamp, p->bias_slope);
    cmd->pipeline = p;
    apply_stencil(cmd);
}

void dkp_stencil_reference(void *command, uint32_t reference) {
    Command *cmd = command;
    if (!begin(cmd, 16))
        return;
    cmd->stencil_ref = reference;
    apply_stencil(cmd);
}

void dkp_blend_constant(void *command, const float *rgba) {
    Command *cmd = command;
    if (!begin(cmd, 16))
        return;
    dkCmdBufSetBlendConst(cmd->buffer, rgba[0], rgba[1], rgba[2], rgba[3]);
}

void dkp_viewport(void *command, float x, float y, float w, float h, float min, float max) {
    Command *cmd = command;
    if (!require(cmd, cmd->in_pass) || !begin(cmd, 16))
        return;
    const DkViewport viewport = {x, y, w, h, min, max};
    dkCmdBufSetViewports(cmd->buffer, 0, &viewport, 1);
}

void dkp_scissor(void *command, uint32_t x, uint32_t y, uint32_t w, uint32_t h) {
    Command *cmd = command;
    if (!require(cmd, cmd->in_pass) || !begin(cmd, 8))
        return;
    const DkScissor scissor = {x, y, w, h};
    dkCmdBufSetScissors(cmd->buffer, 0, &scissor, 1);
}

void dkp_vertex_buffer(void *command, uint32_t slot, uint64_t address, uint32_t size) {
    Command *cmd = command;
    if (!require(cmd, slot < DK_MAX_VERTEX_BUFFERS) || !begin(cmd, 8))
        return;
    dkCmdBufBindVtxBuffer(cmd->buffer, slot, address, size);
}

void dkp_index_buffer(void *command, uint32_t format, uint64_t address) {
    Command *cmd = command;
    if (!begin(cmd, 8))
        return;
    dkCmdBufBindIdxBuffer(cmd->buffer, format, address);
}

// stages: DkStageFlag bits, graphics stages and compute. deko3d keeps bindings per stage.
// Reservations cover all six stages.
void dkp_uniform_buffer(void *command, uint32_t stages, uint32_t slot, uint64_t address, uint32_t size) {
    Command *cmd = command;
    if (!require(cmd, slot < DK_NUM_UNIFORM_BUFS) || !begin(cmd, 6 * 16))
        return;
    for (uint32_t stage = 0; stage <= DkStage_Compute; ++stage)
        if (stages >> stage & 1)
            dkCmdBufBindUniformBuffer(cmd->buffer, stage, slot, address, size);
}

void dkp_storage_buffer(void *command, uint32_t stages, uint32_t slot, uint64_t address, uint32_t size) {
    Command *cmd = command;
    if (!require(cmd, slot < DK_NUM_STORAGE_BUFS) || !begin(cmd, 6 * 16))
        return;
    for (uint32_t stage = 0; stage <= DkStage_Compute; ++stage)
        if (stages >> stage & 1)
            dkCmdBufBindStorageBuffer(cmd->buffer, stage, slot, address, size);
}

void dkp_textures(void *command, uint32_t stages, uint32_t first, const uint32_t *handles, uint32_t count) {
    Command *cmd = command;
    if (!require(cmd, handles && first <= DK_NUM_TEXTURE_BINDINGS && count <= DK_NUM_TEXTURE_BINDINGS - first) ||
        !begin(cmd, 6 * (count + 16)))
        return;
    for (uint32_t stage = 0; stage <= DkStage_Compute; ++stage)
        if (stages >> stage & 1)
            dkCmdBufBindTextures(cmd->buffer, stage, first, handles, count);
}

// Storage images: handles are dkMakeImageHandle(load/store descriptor id).
void dkp_images(void *command, uint32_t stages, uint32_t first, const uint32_t *handles, uint32_t count) {
    Command *cmd = command;
    if (!require(cmd, handles && first <= DK_NUM_IMAGE_BINDINGS && count <= DK_NUM_IMAGE_BINDINGS - first) ||
        !begin(cmd, 6 * (count + 16)))
        return;
    for (uint32_t stage = 0; stage <= DkStage_Compute; ++stage)
        if (stages >> stage & 1)
            dkCmdBufBindImages(cmd->buffer, stage, first, handles, count);
}

// Copies `size` bytes into this command list's upload arena, aligned for uniform buffers.
// Returns the GPU address, or DK_GPU_ADDR_INVALID. The arena lives until the list is freed.
uint64_t dkp_upload(void *command, const void *data, uint32_t size) {
    Command *cmd = command;
    const uint32_t arena_size = 1u << (slot_min_shift + arena_class);
    const uint32_t aligned = (size + DK_UNIFORM_BUF_ALIGNMENT - 1) & ~(DK_UNIFORM_BUF_ALIGNMENT - 1);
    if (!require(cmd, data && size && aligned <= arena_size))
        return DK_GPU_ADDR_INVALID;
    if (!cmd->arena_count || cmd->arena_used + aligned > arena_size) {
        Slot *slot = cmd->arena_count < 16 ? take_slot(cmd->ctx, 1, arena_class) : NULL;
        if (!require(cmd, slot != NULL))
            return DK_GPU_ADDR_INVALID;
        cmd->arena[cmd->arena_count++] = slot;
        cmd->arena_used = 0;
    }
    Slot *slot = cmd->arena[cmd->arena_count - 1];
    const uint32_t offset = slot->offset + cmd->arena_used;
    memcpy((uint8_t *)dkMemBlockGetCpuAddr(slot->memory) + offset, data, size);
    cmd->arena_used += aligned;
    return dkMemBlockGetGpuAddr(slot->memory) + offset;
}

void dkp_draw(void *command, uint32_t vertices, uint32_t instances, uint32_t first_vertex, uint32_t first_instance) {
    Command *cmd = command;
    if (!require(cmd, cmd->in_pass && cmd->pipeline) || !vertices || !instances || !begin(cmd, 16))
        return;
    dkCmdBufDraw(cmd->buffer, cmd->pipeline->primitive, vertices, instances, first_vertex, first_instance);
}

void dkp_draw_indexed(void *command, uint32_t indices, uint32_t instances, uint32_t first_index,
                      int32_t base_vertex, uint32_t first_instance) {
    Command *cmd = command;
    if (!require(cmd, cmd->in_pass && cmd->pipeline) || !indices || !instances || !begin(cmd, 16))
        return;
    dkCmdBufDrawIndexed(cmd->buffer, cmd->pipeline->primitive, indices, instances, first_index, base_vertex,
                        first_instance);
}

void dkp_draw_indirect(void *command, bool indexed, uint64_t address, uint32_t count, uint32_t stride) {
    Command *cmd = command;
    if (!require(cmd, cmd->in_pass && cmd->pipeline))
        return;
    for (uint32_t i = 0; i < count && begin(cmd, 16); ++i, address += stride) {
        if (indexed)
            dkCmdBufDrawIndexedIndirect(cmd->buffer, cmd->pipeline->primitive, address);
        else
            dkCmdBufDrawIndirect(cmd->buffer, cmd->pipeline->primitive, address);
        cmd->entries += 2; // the draw, then its parameters (maxwell/gpu_3d_base.cpp)
    }
}

void dkp_bind_compute(void *command, void *pipeline_ptr) {
    Command *cmd = command;
    ComputePipeline *p = pipeline_ptr;
    if (!require(cmd, !cmd->in_pass && p && p->ctx == cmd->ctx) || !begin(cmd, 64))
        return;
    const DkShader *shader = &p->shader;
    dkCmdBufBindShaders(cmd->buffer, DkStageFlag_Compute, &shader, 1);
}

void dkp_dispatch(void *command, uint32_t x, uint32_t y, uint32_t z) {
    Command *cmd = command;
    if (!require(cmd, !cmd->in_pass) || !x || !y || !z || !begin(cmd, 64))
        return;
    dkCmdBufDispatchCompute(cmd->buffer, x, y, z);
    cmd->entries += 2; // a control command here, and the queue's own commands for the job
}

void dkp_dispatch_indirect(void *command, uint64_t address) {
    Command *cmd = command;
    if (!require(cmd, !cmd->in_pass) || !begin(cmd, 64))
        return;
    dkCmdBufDispatchComputeIndirect(cmd->buffer, address);
    cmd->entries += 8; // as dkp_dispatch, and each of the three parameters (maxwell/gpu_compute.cpp)
}

// Queries: counter reports are 16 bytes, {u64 value, u64 timestamp}, 16-byte aligned.
void dkp_report(void *command, uint32_t counter, uint64_t address) {
    Command *cmd = command;
    if (!require(cmd, !(address & 15)) || !begin(cmd, 16))
        return;
    dkCmdBufReportCounter(cmd->buffer, counter, address);
}

void dkp_reset_counter(void *command, uint32_t counter) {
    Command *cmd = command;
    if (!begin(cmd, 8))
        return;
    dkCmdBufResetCounter(cmd->buffer, counter);
}

// Copies and barriers

void dkp_copy_buffer(void *command, uint64_t from, uint64_t to, uint64_t size) {
    Command *cmd = command;
    if (!require(cmd, !cmd->in_pass && !((from | to | size) & 3)))
        return;
    while (size && begin(cmd, 16)) {
        const uint32_t chunk = size > copy_chunk_size ? copy_chunk_size : (uint32_t)size;
        dkCmdBufCopyBuffer(cmd->buffer, from, to, chunk);
        from += chunk;
        to += chunk;
        size -= chunk;
    }
}

void dkp_clear_buffer(void *command, uint64_t to, uint64_t size) {
    Command *cmd = command;
    if (!require(cmd, !cmd->in_pass && !((to | size) & 3)))
        return;
    const DkGpuAddr zeroes = dkMemBlockGetGpuAddr(cmd->ctx->zeroes);
    while (size && begin(cmd, 16)) {
        const uint32_t chunk = size > zero_block_size ? zero_block_size : (uint32_t)size;
        dkCmdBufCopyBuffer(cmd->buffer, zeroes, to, chunk);
        to += chunk;
        size -= chunk;
    }
}

static void region_view(DkImageView *view, const Texture *texture, const DkpRegion *region, DkImageRect *rect) {
    dkImageViewDefaults(view, &texture->image);
    view->mipLevelOffset = region->mip;
    // z is the first array layer, or the first slice of a 3D image; deko3d treats both as depth.
    *rect = (DkImageRect){region->x, region->y, region->z, region->width, region->height, region->depth};
}

// row_bytes and layer_bytes describe the buffer side; 0 means tightly packed.
void dkp_copy_buffer_image(void *command, bool to_image, uint64_t address, uint32_t row_bytes, uint32_t layer_bytes,
                           void *texture_ptr, const DkpRegion *region) {
    Command *cmd = command;
    Texture *texture = texture_ptr;
    if (!require(cmd, !cmd->in_pass && texture && region && region->width && region->height && region->depth) ||
        !begin(cmd, 96))
        return;
    DkImageView view;
    DkImageRect rect;
    region_view(&view, texture, region, &rect);
    const DkCopyBuf buf = {address, row_bytes, layer_bytes};
    if (to_image)
        dkCmdBufCopyBufferToImage(cmd->buffer, &buf, &view, &rect, 0);
    else
        dkCmdBufCopyImageToBuffer(cmd->buffer, &view, &rect, &buf, 0);
}

void dkp_copy_image(void *command, void *src_ptr, const DkpRegion *src_region, void *dst_ptr,
                    const DkpRegion *dst_region) {
    Command *cmd = command;
    Texture *src = src_ptr;
    Texture *dst = dst_ptr;
    if (!require(cmd, !cmd->in_pass && src && dst && src_region && dst_region) || !begin(cmd, 128))
        return;
    DkImageView src_view, dst_view;
    DkImageRect src_rect, dst_rect;
    region_view(&src_view, src, src_region, &src_rect);
    region_view(&dst_view, dst, dst_region, &dst_rect);
    dkCmdBufCopyImage(cmd->buffer, &src_view, &src_rect, &dst_view, &dst_rect, 0);
}

// invalidate: DkInvalidateFlags bits for what the next commands read. The barrier is recorded
// before the next command, so back-to-back barriers (wgpu-core transitions buffers, then
// textures) cost one.
void dkp_barrier(void *command, uint32_t invalidate) {
    Command *cmd = command;
    if (!require(cmd, true))
        return;
    cmd->barrier_pending = true;
    cmd->barrier_invalidate |= invalidate;
}

// Presentation

// Clears the images to opaque black. A frame presented before anything draws to it (Bevy's first,
// while its blit pipeline compiles) otherwise shows whatever the memory last held, such as a frame
// of the previous app. Best effort: if this fails, the images keep their old contents.
static void clear_swapchain_images(Context *ctx, const DkImage *const *images, uint32_t count, uint32_t width,
                                   uint32_t height) {
    DkMemBlock memory = make_memory(ctx->device, DK_MEMBLOCK_ALIGNMENT, gpu_flags);
    if (!memory)
        return;
    DkCmdBufMaker maker;
    dkCmdBufMakerDefaults(&maker, ctx->device);
    DkCmdBuf buffer = dkCmdBufCreate(&maker);
    if (buffer) {
        dkCmdBufAddMemory(buffer, memory, 0, DK_MEMBLOCK_ALIGNMENT);
        const DkViewport viewport = {0, 0, (float)width, (float)height, 0, 1};
        const DkScissor scissor = {0, 0, width, height};
        for (uint32_t i = 0; i < count; ++i) {
            DkImageView view;
            dkImageViewDefaults(&view, images[i]);
            dkCmdBufBindRenderTarget(buffer, &view, NULL);
            dkCmdBufSetViewports(buffer, 0, &viewport, 1);
            dkCmdBufSetScissors(buffer, 0, &scissor, 1);
            dkCmdBufClearColorFloat(buffer, 0, DkColorMask_RGBA, 0.0f, 0.0f, 0.0f, 1.0f);
        }
        DkCmdList list = dkCmdBufFinishList(buffer);
        mutexLock(&ctx->queue_lock);
        if (queue_healthy_locked(ctx)) {
            dkQueueSubmitCommands(ctx->queue, list);
            dkQueueWaitIdle(ctx->queue);
            queue_healthy_locked(ctx); // marks the queue lost if the clear faulted
        }
        mutexUnlock(&ctx->queue_lock);
        dkCmdBufDestroy(buffer);
    }
    destroy_memory(memory);
}

void *dkp_swapchain(void *ptr, void *native_window, void *const *textures, uint32_t count, uint32_t width,
                    uint32_t height) {
    Context *ctx = ptr;
    if (!ctx || !native_window || !textures || !count || count > 8 || !width || !height)
        return NULL;
    const DkImage *images[8];
    for (uint32_t i = 0; i < count; ++i)
        images[i] = &((const Texture *)textures[i])->image;
    clear_swapchain_images(ctx, images, count, width, height);
    Swapchain *swapchain = malloc(sizeof(*swapchain));
    if (!swapchain)
        return NULL;
    DkSwapchainMaker maker;
    dkSwapchainMakerDefaults(&maker, ctx->device, native_window, images, count);
    swapchain->ctx = ctx;
    swapchain->native = dkSwapchainCreate(&maker);
    if (!swapchain->native) {
        free(swapchain);
        return NULL;
    }
    return swapchain;
}

// The caller makes sure no submitted work still renders to the swapchain's images.
void dkp_swapchain_free(void *ptr) {
    Swapchain *swapchain = ptr;
    if (!swapchain)
        return;
    Context *ctx = swapchain->ctx;
    mutexLock(&ctx->queue_lock);
    if (ctx->queue)
        dkQueueWaitIdle(ctx->queue);
    mutexUnlock(&ctx->queue_lock);
    dkSwapchainDestroy(swapchain->native);
    free(swapchain);
}

void dkp_swapchain_interval(void *ptr, uint32_t interval) {
    Swapchain *swapchain = ptr;
    if (swapchain)
        dkSwapchainSetSwapInterval(swapchain->native, interval);
}

// Blocks the CPU until the compositor returns an image; the queue then waits on the GPU
// for the image to be released before running later submissions. Returns the slot, or -1.
int dkp_acquire(void *ptr) {
    Swapchain *swapchain = ptr;
    if (!swapchain)
        return -1;
    int slot = -1;
    DkFence fence;
    const uint64_t started = armGetSystemTick();
    dkSwapchainAcquireImage(swapchain->native, &slot, &fence);
    stat_end(stat_acquire, started);
    Context *ctx = swapchain->ctx;
    lock_queue(ctx);
    bool ok = slot >= 0 && queue_healthy_locked(ctx);
    if (ok)
        dkQueueWaitFence(ctx->queue, &fence);
    mutexUnlock(&ctx->queue_lock);
    return ok ? slot : -1;
}

bool dkp_present(void *ptr, int slot) {
    Swapchain *swapchain = ptr;
    if (!swapchain || slot < 0)
        return false;
    Context *ctx = swapchain->ctx;
    lock_queue(ctx);
    bool ok = queue_healthy_locked(ctx);
    if (ok) {
        const uint64_t started = armGetSystemTick();
        dkQueuePresentImage(ctx->queue, swapchain->native, slot);
        stat_end(stat_present, started);
        ok = queue_healthy_locked(ctx);
    }
    mutexUnlock(&ctx->queue_lock);
    return ok;
}
