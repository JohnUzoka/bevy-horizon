// nvk_probe: checks NVK, Mesa's Vulkan driver from danfromtico/mesa-switch, on the Switch without
// Rust or Bevy in the way. It logs what wgpu and Bevy would care about (device, limits, features,
// formats, the Vi surface) to sdmc:/switch/nvk_probe/log.txt, then draws a rotating triangle
// through the Vi WSI for 30 seconds or until + is pressed.
//
// Written for bevy-horizon. The Vulkan calls follow the usual order from the Vulkan spec and
// Khronos samples; nothing is copied from them. The surface is VK_NN_vi_surface on libnx's default
// NWindow, as mesa-switch's src/vulkan/wsi/wsi_common_switch.c expects.
#include <inttypes.h>
#include <math.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>

#include <switch.h>

#define VK_USE_PLATFORM_VI_NN
#include <vulkan/vulkan.h>

#include "tri_frag.h"
#include "tri_vert.h"

#define RUN_SECONDS 30
#define MAX_IMAGES 8

static FILE *g_log;
static u64 g_start;

static double seconds(void) { return armTicksToNs(armGetSystemTick() - g_start) / 1e9; }

static void say(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    if (g_log) {
        fprintf(g_log, "%7.3f ", seconds());
        vfprintf(g_log, fmt, ap);
        fputc('\n', g_log);
        fflush(g_log);
    }
    va_end(ap);
}

#define CHECK(call)                                                        \
    do {                                                                   \
        VkResult r_ = (call);                                              \
        if (r_ != VK_SUCCESS) {                                            \
            say("FAIL %s = %d (line %d)", #call, (int)r_, __LINE__);      \
            goto fail;                                                     \
        }                                                                  \
    } while (0)

static const char *yes(VkBool32 b) { return b ? "yes" : "no"; }

static VKAPI_ATTR VkBool32 VKAPI_CALL
driver_message(VkDebugUtilsMessageSeverityFlagBitsEXT severity,
               VkDebugUtilsMessageTypeFlagsEXT types,
               const VkDebugUtilsMessengerCallbackDataEXT *data, void *user) {
    const char *level = severity >= VK_DEBUG_UTILS_MESSAGE_SEVERITY_ERROR_BIT_EXT     ? "error"
                        : severity >= VK_DEBUG_UTILS_MESSAGE_SEVERITY_WARNING_BIT_EXT ? "warning"
                        : severity >= VK_DEBUG_UTILS_MESSAGE_SEVERITY_INFO_BIT_EXT    ? "info"
                                                                                       : "debug";
    say("driver %s: %s", level, data->pMessage ? data->pMessage : "(no message)");
    return VK_FALSE;
}

static void log_features(VkPhysicalDevice pd) {
    VkPhysicalDeviceFeatures f;
    vkGetPhysicalDeviceFeatures(pd, &f);
    say("features: BC %s, ETC2 %s, ASTC_LDR %s, geometryShader %s, tessellation %s, "
        "samplerAnisotropy %s, independentBlend %s, dualSrcBlend %s, multiDrawIndirect %s, "
        "drawIndirectFirstInstance %s, depthClamp %s, depthBiasClamp %s, fillModeNonSolid %s, "
        "shaderInt64 %s, shaderFloat64 %s, shaderInt16 %s, fragmentStoresAndAtomics %s, "
        "vertexPipelineStoresAndAtomics %s, imageCubeArray %s, sampleRateShading %s, "
        "fullDrawIndexUint32 %s, robustBufferAccess %s, occlusionQueryPrecise %s, "
        "pipelineStatisticsQuery %s, shaderClipDistance %s",
        yes(f.textureCompressionBC), yes(f.textureCompressionETC2),
        yes(f.textureCompressionASTC_LDR), yes(f.geometryShader), yes(f.tessellationShader),
        yes(f.samplerAnisotropy), yes(f.independentBlend), yes(f.dualSrcBlend),
        yes(f.multiDrawIndirect), yes(f.drawIndirectFirstInstance), yes(f.depthClamp),
        yes(f.depthBiasClamp), yes(f.fillModeNonSolid), yes(f.shaderInt64),
        yes(f.shaderFloat64), yes(f.shaderInt16), yes(f.fragmentStoresAndAtomics),
        yes(f.vertexPipelineStoresAndAtomics), yes(f.imageCubeArray), yes(f.sampleRateShading),
        yes(f.fullDrawIndexUint32), yes(f.robustBufferAccess), yes(f.occlusionQueryPrecise),
        yes(f.pipelineStatisticsQuery), yes(f.shaderClipDistance));
}

static void log_limits(const VkPhysicalDeviceLimits *l) {
    say("limits: maxImageDimension2D %u, maxImageDimensionCube %u, maxImageArrayLayers %u, "
        "maxBoundDescriptorSets %u, maxPerStageDescriptorSamplers %u, "
        "maxPerStageDescriptorUniformBuffers %u, maxPerStageDescriptorStorageBuffers %u, "
        "maxPerStageDescriptorSampledImages %u, maxPerStageDescriptorStorageImages %u, "
        "maxUniformBufferRange %u, maxStorageBufferRange %u, maxPushConstantsSize %u, "
        "maxVertexInputAttributes %u, maxVertexInputBindings %u, maxColorAttachments %u, "
        "maxComputeWorkGroupInvocations %u, maxComputeWorkGroupSize %ux%ux%u, "
        "maxComputeSharedMemorySize %u, minUniformBufferOffsetAlignment %" PRIu64 ", "
        "minStorageBufferOffsetAlignment %" PRIu64 ", maxSamplerAnisotropy %.0f, "
        "timestampPeriod %.2f ns, framebufferColorSampleCounts 0x%x",
        l->maxImageDimension2D, l->maxImageDimensionCube, l->maxImageArrayLayers,
        l->maxBoundDescriptorSets, l->maxPerStageDescriptorSamplers,
        l->maxPerStageDescriptorUniformBuffers, l->maxPerStageDescriptorStorageBuffers,
        l->maxPerStageDescriptorSampledImages, l->maxPerStageDescriptorStorageImages,
        l->maxUniformBufferRange, l->maxStorageBufferRange, l->maxPushConstantsSize,
        l->maxVertexInputAttributes, l->maxVertexInputBindings, l->maxColorAttachments,
        l->maxComputeWorkGroupInvocations, l->maxComputeWorkGroupSize[0],
        l->maxComputeWorkGroupSize[1], l->maxComputeWorkGroupSize[2],
        l->maxComputeSharedMemorySize, (uint64_t)l->minUniformBufferOffsetAlignment,
        (uint64_t)l->minStorageBufferOffsetAlignment, l->maxSamplerAnisotropy,
        l->timestampPeriod, l->framebufferColorSampleCounts);
}

static void log_formats(VkPhysicalDevice pd) {
    static const struct {
        VkFormat format;
        const char *name;
    } formats[] = {
        {VK_FORMAT_R8G8B8A8_UNORM, "R8G8B8A8_UNORM"},
        {VK_FORMAT_R8G8B8A8_SRGB, "R8G8B8A8_SRGB"},
        {VK_FORMAT_B8G8R8A8_UNORM, "B8G8R8A8_UNORM"},
        {VK_FORMAT_B8G8R8A8_SRGB, "B8G8R8A8_SRGB"},
        {VK_FORMAT_R16G16B16A16_SFLOAT, "R16G16B16A16_SFLOAT (Bevy HDR)"},
        {VK_FORMAT_B10G11R11_UFLOAT_PACK32, "B10G11R11_UFLOAT"},
        {VK_FORMAT_A2B10G10R10_UNORM_PACK32, "A2B10G10R10_UNORM"},
        {VK_FORMAT_R32G32B32A32_SFLOAT, "R32G32B32A32_SFLOAT"},
        {VK_FORMAT_D32_SFLOAT, "D32_SFLOAT (Bevy depth)"},
        {VK_FORMAT_D24_UNORM_S8_UINT, "D24_UNORM_S8_UINT"},
        {VK_FORMAT_D32_SFLOAT_S8_UINT, "D32_SFLOAT_S8_UINT"},
        {VK_FORMAT_BC1_RGBA_SRGB_BLOCK, "BC1_SRGB"},
        {VK_FORMAT_BC3_SRGB_BLOCK, "BC3_SRGB"},
        {VK_FORMAT_BC5_UNORM_BLOCK, "BC5_UNORM"},
        {VK_FORMAT_BC7_SRGB_BLOCK, "BC7_SRGB"},
        {VK_FORMAT_ASTC_4x4_SRGB_BLOCK, "ASTC_4x4_SRGB"},
        {VK_FORMAT_ETC2_R8G8B8A8_SRGB_BLOCK, "ETC2_RGBA8_SRGB"},
    };
    for (size_t i = 0; i < sizeof formats / sizeof formats[0]; i++) {
        VkFormatProperties p;
        vkGetPhysicalDeviceFormatProperties(pd, formats[i].format, &p);
        VkFormatFeatureFlags f = p.optimalTilingFeatures;
        say("format %s: optimal 0x%x (sampled %s, filter %s, color attachment %s, blend %s, "
            "depth %s, storage %s)",
            formats[i].name, f, yes(f & VK_FORMAT_FEATURE_SAMPLED_IMAGE_BIT),
            yes(f & VK_FORMAT_FEATURE_SAMPLED_IMAGE_FILTER_LINEAR_BIT),
            yes(f & VK_FORMAT_FEATURE_COLOR_ATTACHMENT_BIT),
            yes(f & VK_FORMAT_FEATURE_COLOR_ATTACHMENT_BLEND_BIT),
            yes(f & VK_FORMAT_FEATURE_DEPTH_STENCIL_ATTACHMENT_BIT),
            yes(f & VK_FORMAT_FEATURE_STORAGE_IMAGE_BIT));
    }
}

static VkShaderModule shader(VkDevice dev, const uint32_t *code, size_t size) {
    VkShaderModuleCreateInfo info = {
        .sType = VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO,
        .codeSize = size,
        .pCode = code,
    };
    VkShaderModule module = VK_NULL_HANDLE;
    VkResult r = vkCreateShaderModule(dev, &info, NULL, &module);
    if (r != VK_SUCCESS)
        say("FAIL vkCreateShaderModule = %d", (int)r);
    return module;
}

// NAK, NVK's shader compiler, is Rust built for aarch64-unknown-linux-gnu: it finds its
// thread-locals at TPIDR_EL0 plus a link-time offset. libnx never writes TPIDR_EL0 and keeps the
// thread pointer in ThreadVars.tls_tp instead (where devkitA64's -mtp=soft C code reads it), so
// copy it over on every thread before any Rust code runs. bevy_horizon does the same for Rust apps
// (crates/bevy_horizon/src/compat.rs, set_thread_pointer). ThreadVars is libnx-internal: the last
// 0x20 bytes of the thread's TLS page, with tls_tp at 0x1F8.
static void set_thread_pointer(void) {
    void *tls_tp = *(void **)((u8 *)armGetTls() + 0x1F8);
    __asm__ volatile("msr tpidr_el0, %0" ::"r"(tls_tp));
}

typedef struct {
    ThreadFunc entry;
    void *arg;
} ThreadStart;

Result __real_threadCreate(Thread *t, ThreadFunc entry, void *arg, void *stack_mem,
                           size_t stack_sz, int prio, int cpuid);

static void thread_start(void *p) {
    ThreadStart start = *(ThreadStart *)p;
    free(p);
    set_thread_pointer();
    start.entry(start.arg);
}

// Linked with -Wl,--wrap=threadCreate, so this also sees the threads Mesa starts through pthreads.
Result __wrap_threadCreate(Thread *t, ThreadFunc entry, void *arg, void *stack_mem,
                           size_t stack_sz, int prio, int cpuid) {
    ThreadStart *start = malloc(sizeof *start);
    if (!start)
        return MAKERESULT(Module_Libnx, LibnxError_OutOfMemory);
    start->entry = entry;
    start->arg = arg;
    Result rc = __real_threadCreate(t, thread_start, start, stack_mem, stack_sz, prio, cpuid);
    if (R_FAILED(rc))
        free(start);
    say("thread created by the driver: rc 0x%x, priority 0x%x, core %d", rc, prio, cpuid);
    return rc;
}

int main(int argc, char **argv) {
    (void)argc;
    (void)argv;
    set_thread_pointer();
    g_start = armGetSystemTick();
    // Mesa's own messages (vk_errorf and the nvkmd-switch layer) go to stderr, which nothing reads
    // on a Switch; point them at a file instead. Read once, at the first message.
    setenv("MESA_LOG_FILE", "sdmc:/switch/nvk_probe/mesa.txt", 1);
    setenv("MESA_LOG_LEVEL", "debug", 1);
    mkdir("sdmc:/switch", 0777);
    mkdir("sdmc:/switch/nvk_probe", 0777);
    g_log = fopen("sdmc:/switch/nvk_probe/log.txt", "w");
    say("nvk_probe started");

    padConfigureInput(1, HidNpadStyleSet_NpadStandard);
    PadState pad;
    padInitializeDefault(&pad);

    VkInstance instance = VK_NULL_HANDLE;
    VkDebugUtilsMessengerEXT messenger = VK_NULL_HANDLE;
    VkDevice dev = VK_NULL_HANDLE;
    VkSurfaceKHR surface = VK_NULL_HANDLE;
    VkSwapchainKHR swapchain = VK_NULL_HANDLE;
    VkRenderPass render_pass = VK_NULL_HANDLE;
    VkPipelineLayout layout = VK_NULL_HANDLE;
    VkPipeline pipeline = VK_NULL_HANDLE;
    VkCommandPool pool = VK_NULL_HANDLE;
    VkSemaphore acquired = VK_NULL_HANDLE;
    VkSemaphore rendered[MAX_IMAGES] = {0};
    VkFence fence = VK_NULL_HANDLE;
    VkImageView views[MAX_IMAGES] = {0};
    VkFramebuffer framebuffers[MAX_IMAGES] = {0};
    VkShaderModule vert = VK_NULL_HANDLE, frag = VK_NULL_HANDLE;
    uint32_t image_count = 0;
    int ok = 0;

    uint32_t api = 0;
    CHECK(vkEnumerateInstanceVersion(&api));
    say("instance version %u.%u.%u", VK_API_VERSION_MAJOR(api), VK_API_VERSION_MINOR(api),
        VK_API_VERSION_PATCH(api));

    uint32_t n = 0;
    CHECK(vkEnumerateInstanceExtensionProperties(NULL, &n, NULL));
    VkExtensionProperties *exts = calloc(n, sizeof *exts);
    CHECK(vkEnumerateInstanceExtensionProperties(NULL, &n, exts));
    for (uint32_t i = 0; i < n; i++)
        say("instance extension %s v%u", exts[i].extensionName, exts[i].specVersion);
    free(exts);

    const char *instance_exts[] = {VK_KHR_SURFACE_EXTENSION_NAME, VK_NN_VI_SURFACE_EXTENSION_NAME,
                                   VK_EXT_DEBUG_UTILS_EXTENSION_NAME};
    VkApplicationInfo app = {
        .sType = VK_STRUCTURE_TYPE_APPLICATION_INFO,
        .pApplicationName = "nvk_probe",
        .apiVersion = VK_API_VERSION_1_1,
    };
    VkDebugUtilsMessengerCreateInfoEXT messenger_info = {
        .sType = VK_STRUCTURE_TYPE_DEBUG_UTILS_MESSENGER_CREATE_INFO_EXT,
        .messageSeverity = VK_DEBUG_UTILS_MESSAGE_SEVERITY_VERBOSE_BIT_EXT |
                           VK_DEBUG_UTILS_MESSAGE_SEVERITY_INFO_BIT_EXT |
                           VK_DEBUG_UTILS_MESSAGE_SEVERITY_WARNING_BIT_EXT |
                           VK_DEBUG_UTILS_MESSAGE_SEVERITY_ERROR_BIT_EXT,
        .messageType = VK_DEBUG_UTILS_MESSAGE_TYPE_GENERAL_BIT_EXT |
                       VK_DEBUG_UTILS_MESSAGE_TYPE_VALIDATION_BIT_EXT |
                       VK_DEBUG_UTILS_MESSAGE_TYPE_PERFORMANCE_BIT_EXT,
        .pfnUserCallback = driver_message,
    };
    VkInstanceCreateInfo instance_info = {
        .sType = VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO,
        // Messages from vkCreateInstance itself.
        .pNext = &messenger_info,
        .pApplicationInfo = &app,
        .enabledExtensionCount = 3,
        .ppEnabledExtensionNames = instance_exts,
    };
    u64 t0 = armGetSystemTick();
    CHECK(vkCreateInstance(&instance_info, NULL, &instance));
    say("vkCreateInstance ok in %.1f ms", armTicksToNs(armGetSystemTick() - t0) / 1e6);

    // A release Mesa drops its error messages (the reason behind a failing call) unless the app
    // has a debug messenger.
    PFN_vkCreateDebugUtilsMessengerEXT create_messenger =
        (PFN_vkCreateDebugUtilsMessengerEXT)vkGetInstanceProcAddr(
            instance, "vkCreateDebugUtilsMessengerEXT");
    if (create_messenger)
        CHECK(create_messenger(instance, &messenger_info, NULL, &messenger));
    else
        say("no vkCreateDebugUtilsMessengerEXT; driver messages will be missing");

    uint32_t pd_count = 0;
    CHECK(vkEnumeratePhysicalDevices(instance, &pd_count, NULL));
    say("%u physical device(s)", pd_count);
    if (pd_count == 0)
        goto fail;
    VkPhysicalDevice pd;
    pd_count = 1;
    VkResult enum_r = vkEnumeratePhysicalDevices(instance, &pd_count, &pd);
    if (enum_r != VK_SUCCESS && enum_r != VK_INCOMPLETE) {
        say("FAIL vkEnumeratePhysicalDevices = %d", (int)enum_r);
        goto fail;
    }

    VkPhysicalDeviceProperties props;
    vkGetPhysicalDeviceProperties(pd, &props);
    say("device \"%s\", type %d, vendor 0x%04x, device 0x%04x, API %u.%u.%u, driver 0x%x",
        props.deviceName, (int)props.deviceType, props.vendorID, props.deviceID,
        VK_API_VERSION_MAJOR(props.apiVersion), VK_API_VERSION_MINOR(props.apiVersion),
        VK_API_VERSION_PATCH(props.apiVersion), props.driverVersion);
    if (props.apiVersion >= VK_API_VERSION_1_2) {
        VkPhysicalDeviceDriverProperties driver = {
            .sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_DRIVER_PROPERTIES,
        };
        VkPhysicalDeviceProperties2 props2 = {
            .sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_PROPERTIES_2,
            .pNext = &driver,
        };
        vkGetPhysicalDeviceProperties2(pd, &props2);
        say("driver %s (%s), conformance %u.%u.%u.%u", driver.driverName, driver.driverInfo,
            driver.conformanceVersion.major, driver.conformanceVersion.minor,
            driver.conformanceVersion.subminor, driver.conformanceVersion.patch);
    }
    log_limits(&props.limits);
    log_features(pd);
    log_formats(pd);

    VkPhysicalDeviceMemoryProperties mem;
    vkGetPhysicalDeviceMemoryProperties(pd, &mem);
    for (uint32_t i = 0; i < mem.memoryHeapCount; i++)
        say("memory heap %u: %.1f MiB, flags 0x%x", i, mem.memoryHeaps[i].size / 1048576.0,
            mem.memoryHeaps[i].flags);
    for (uint32_t i = 0; i < mem.memoryTypeCount; i++)
        say("memory type %u: heap %u, flags 0x%x", i, mem.memoryTypes[i].heapIndex,
            mem.memoryTypes[i].propertyFlags);

    CHECK(vkEnumerateDeviceExtensionProperties(pd, NULL, &n, NULL));
    exts = calloc(n, sizeof *exts);
    CHECK(vkEnumerateDeviceExtensionProperties(pd, NULL, &n, exts));
    say("%u device extensions:", n);
    for (uint32_t i = 0; i < n; i++)
        say("  %s v%u", exts[i].extensionName, exts[i].specVersion);
    free(exts);

    uint32_t qf_count = 0;
    vkGetPhysicalDeviceQueueFamilyProperties(pd, &qf_count, NULL);
    VkQueueFamilyProperties qfs[8];
    if (qf_count > 8)
        qf_count = 8;
    vkGetPhysicalDeviceQueueFamilyProperties(pd, &qf_count, qfs);
    for (uint32_t i = 0; i < qf_count; i++)
        say("queue family %u: flags 0x%x, %u queue(s), timestamp bits %u", i, qfs[i].queueFlags,
            qfs[i].queueCount, qfs[i].timestampValidBits);

    NWindow *win = nwindowGetDefault();
    u32 win_w = 0, win_h = 0;
    nwindowGetDimensions(win, &win_w, &win_h);
    say("default NWindow %p, %ux%u", (void *)win, win_w, win_h);
    VkViSurfaceCreateInfoNN surface_info = {
        .sType = VK_STRUCTURE_TYPE_VI_SURFACE_CREATE_INFO_NN,
        .window = win,
    };
    CHECK(vkCreateViSurfaceNN(instance, &surface_info, NULL, &surface));

    uint32_t family = UINT32_MAX;
    for (uint32_t i = 0; i < qf_count; i++) {
        VkBool32 present = VK_FALSE;
        CHECK(vkGetPhysicalDeviceSurfaceSupportKHR(pd, i, surface, &present));
        say("queue family %u presents: %s", i, yes(present));
        if (present && (qfs[i].queueFlags & VK_QUEUE_GRAPHICS_BIT) && family == UINT32_MAX)
            family = i;
    }
    if (family == UINT32_MAX) {
        say("FAIL no queue family both draws and presents");
        goto fail;
    }

    VkSurfaceCapabilitiesKHR caps;
    CHECK(vkGetPhysicalDeviceSurfaceCapabilitiesKHR(pd, surface, &caps));
    say("surface: images %u..%u, extent %ux%u (min %ux%u, max %ux%u), usage 0x%x, "
        "transforms 0x%x, composite alpha 0x%x",
        caps.minImageCount, caps.maxImageCount, caps.currentExtent.width,
        caps.currentExtent.height, caps.minImageExtent.width, caps.minImageExtent.height,
        caps.maxImageExtent.width, caps.maxImageExtent.height, caps.supportedUsageFlags,
        caps.supportedTransforms, caps.supportedCompositeAlpha);

    uint32_t format_count = 0;
    CHECK(vkGetPhysicalDeviceSurfaceFormatsKHR(pd, surface, &format_count, NULL));
    VkSurfaceFormatKHR formats[16];
    if (format_count > 16)
        format_count = 16;
    CHECK(vkGetPhysicalDeviceSurfaceFormatsKHR(pd, surface, &format_count, formats));
    for (uint32_t i = 0; i < format_count; i++)
        say("surface format %u: format %d, colour space %d", i, (int)formats[i].format,
            (int)formats[i].colorSpace);
    uint32_t mode_count = 0;
    CHECK(vkGetPhysicalDeviceSurfacePresentModesKHR(pd, surface, &mode_count, NULL));
    VkPresentModeKHR modes[8];
    if (mode_count > 8)
        mode_count = 8;
    CHECK(vkGetPhysicalDeviceSurfacePresentModesKHR(pd, surface, &mode_count, modes));
    for (uint32_t i = 0; i < mode_count; i++)
        say("present mode %d", (int)modes[i]);
    if (format_count == 0) {
        say("FAIL the surface has no formats");
        goto fail;
    }

    float priority = 1.0f;
    VkDeviceQueueCreateInfo queue_info = {
        .sType = VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO,
        .queueFamilyIndex = family,
        .queueCount = 1,
        .pQueuePriorities = &priority,
    };
    const char *device_exts[] = {VK_KHR_SWAPCHAIN_EXTENSION_NAME};
    VkDeviceCreateInfo device_info = {
        .sType = VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO,
        .queueCreateInfoCount = 1,
        .pQueueCreateInfos = &queue_info,
        .enabledExtensionCount = 1,
        .ppEnabledExtensionNames = device_exts,
    };
    t0 = armGetSystemTick();
    CHECK(vkCreateDevice(pd, &device_info, NULL, &dev));
    say("vkCreateDevice ok in %.1f ms", armTicksToNs(armGetSystemTick() - t0) / 1e6);
    VkQueue queue;
    vkGetDeviceQueue(dev, family, 0, &queue);

    VkSurfaceFormatKHR chosen = formats[0];
    for (uint32_t i = 0; i < format_count; i++)
        if (formats[i].format == VK_FORMAT_R8G8B8A8_UNORM ||
            formats[i].format == VK_FORMAT_B8G8R8A8_UNORM) {
            chosen = formats[i];
            break;
        }
    VkExtent2D extent = caps.currentExtent;
    if (extent.width == UINT32_MAX)
        extent = (VkExtent2D){win_w ? win_w : 1280, win_h ? win_h : 720};
    VkSwapchainCreateInfoKHR swapchain_info = {
        .sType = VK_STRUCTURE_TYPE_SWAPCHAIN_CREATE_INFO_KHR,
        .surface = surface,
        .minImageCount = caps.minImageCount,
        .imageFormat = chosen.format,
        .imageColorSpace = chosen.colorSpace,
        .imageExtent = extent,
        .imageArrayLayers = 1,
        .imageUsage = VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT,
        .imageSharingMode = VK_SHARING_MODE_EXCLUSIVE,
        .preTransform = caps.currentTransform,
        .compositeAlpha = VK_COMPOSITE_ALPHA_OPAQUE_BIT_KHR,
        .presentMode = VK_PRESENT_MODE_FIFO_KHR,
        .clipped = VK_TRUE,
    };
    t0 = armGetSystemTick();
    CHECK(vkCreateSwapchainKHR(dev, &swapchain_info, NULL, &swapchain));
    CHECK(vkGetSwapchainImagesKHR(dev, swapchain, &image_count, NULL));
    if (image_count > MAX_IMAGES) {
        say("FAIL %u swapchain images", image_count);
        goto fail;
    }
    VkImage images[MAX_IMAGES];
    CHECK(vkGetSwapchainImagesKHR(dev, swapchain, &image_count, images));
    say("swapchain: %u images, format %d, %ux%u, FIFO, in %.1f ms", image_count,
        (int)chosen.format, extent.width, extent.height,
        armTicksToNs(armGetSystemTick() - t0) / 1e6);

    VkAttachmentDescription color = {
        .format = chosen.format,
        .samples = VK_SAMPLE_COUNT_1_BIT,
        .loadOp = VK_ATTACHMENT_LOAD_OP_CLEAR,
        .storeOp = VK_ATTACHMENT_STORE_OP_STORE,
        .stencilLoadOp = VK_ATTACHMENT_LOAD_OP_DONT_CARE,
        .stencilStoreOp = VK_ATTACHMENT_STORE_OP_DONT_CARE,
        .initialLayout = VK_IMAGE_LAYOUT_UNDEFINED,
        .finalLayout = VK_IMAGE_LAYOUT_PRESENT_SRC_KHR,
    };
    VkAttachmentReference color_ref = {0, VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL};
    VkSubpassDescription subpass = {
        .pipelineBindPoint = VK_PIPELINE_BIND_POINT_GRAPHICS,
        .colorAttachmentCount = 1,
        .pColorAttachments = &color_ref,
    };
    VkSubpassDependency dependency = {
        .srcSubpass = VK_SUBPASS_EXTERNAL,
        .dstSubpass = 0,
        .srcStageMask = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT,
        .dstStageMask = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT,
        .dstAccessMask = VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT,
    };
    VkRenderPassCreateInfo rp_info = {
        .sType = VK_STRUCTURE_TYPE_RENDER_PASS_CREATE_INFO,
        .attachmentCount = 1,
        .pAttachments = &color,
        .subpassCount = 1,
        .pSubpasses = &subpass,
        .dependencyCount = 1,
        .pDependencies = &dependency,
    };
    CHECK(vkCreateRenderPass(dev, &rp_info, NULL, &render_pass));

    for (uint32_t i = 0; i < image_count; i++) {
        VkImageViewCreateInfo view_info = {
            .sType = VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO,
            .image = images[i],
            .viewType = VK_IMAGE_VIEW_TYPE_2D,
            .format = chosen.format,
            .subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1},
        };
        CHECK(vkCreateImageView(dev, &view_info, NULL, &views[i]));
        VkFramebufferCreateInfo fb_info = {
            .sType = VK_STRUCTURE_TYPE_FRAMEBUFFER_CREATE_INFO,
            .renderPass = render_pass,
            .attachmentCount = 1,
            .pAttachments = &views[i],
            .width = extent.width,
            .height = extent.height,
            .layers = 1,
        };
        CHECK(vkCreateFramebuffer(dev, &fb_info, NULL, &framebuffers[i]));
        VkSemaphoreCreateInfo sem_info = {.sType = VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO};
        CHECK(vkCreateSemaphore(dev, &sem_info, NULL, &rendered[i]));
    }

    t0 = armGetSystemTick();
    vert = shader(dev, tri_vert_spv, sizeof tri_vert_spv);
    frag = shader(dev, tri_frag_spv, sizeof tri_frag_spv);
    if (!vert || !frag)
        goto fail;
    VkPushConstantRange push = {VK_SHADER_STAGE_VERTEX_BIT, 0, sizeof(float)};
    VkPipelineLayoutCreateInfo layout_info = {
        .sType = VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO,
        .pushConstantRangeCount = 1,
        .pPushConstantRanges = &push,
    };
    CHECK(vkCreatePipelineLayout(dev, &layout_info, NULL, &layout));
    VkPipelineShaderStageCreateInfo stages[2] = {
        {.sType = VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO,
         .stage = VK_SHADER_STAGE_VERTEX_BIT,
         .module = vert,
         .pName = "main"},
        {.sType = VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO,
         .stage = VK_SHADER_STAGE_FRAGMENT_BIT,
         .module = frag,
         .pName = "main"},
    };
    VkPipelineVertexInputStateCreateInfo vertex_input = {
        .sType = VK_STRUCTURE_TYPE_PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO,
    };
    VkPipelineInputAssemblyStateCreateInfo assembly = {
        .sType = VK_STRUCTURE_TYPE_PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO,
        .topology = VK_PRIMITIVE_TOPOLOGY_TRIANGLE_LIST,
    };
    VkPipelineViewportStateCreateInfo viewport = {
        .sType = VK_STRUCTURE_TYPE_PIPELINE_VIEWPORT_STATE_CREATE_INFO,
        .viewportCount = 1,
        .scissorCount = 1,
    };
    VkPipelineRasterizationStateCreateInfo raster = {
        .sType = VK_STRUCTURE_TYPE_PIPELINE_RASTERIZATION_STATE_CREATE_INFO,
        .polygonMode = VK_POLYGON_MODE_FILL,
        .cullMode = VK_CULL_MODE_NONE,
        .frontFace = VK_FRONT_FACE_CLOCKWISE,
        .lineWidth = 1.0f,
    };
    VkPipelineMultisampleStateCreateInfo multisample = {
        .sType = VK_STRUCTURE_TYPE_PIPELINE_MULTISAMPLE_STATE_CREATE_INFO,
        .rasterizationSamples = VK_SAMPLE_COUNT_1_BIT,
    };
    VkPipelineColorBlendAttachmentState blend_attachment = {
        .colorWriteMask = VK_COLOR_COMPONENT_R_BIT | VK_COLOR_COMPONENT_G_BIT |
                          VK_COLOR_COMPONENT_B_BIT | VK_COLOR_COMPONENT_A_BIT,
    };
    VkPipelineColorBlendStateCreateInfo blend = {
        .sType = VK_STRUCTURE_TYPE_PIPELINE_COLOR_BLEND_STATE_CREATE_INFO,
        .attachmentCount = 1,
        .pAttachments = &blend_attachment,
    };
    VkDynamicState dynamic_states[] = {VK_DYNAMIC_STATE_VIEWPORT, VK_DYNAMIC_STATE_SCISSOR};
    VkPipelineDynamicStateCreateInfo dynamic = {
        .sType = VK_STRUCTURE_TYPE_PIPELINE_DYNAMIC_STATE_CREATE_INFO,
        .dynamicStateCount = 2,
        .pDynamicStates = dynamic_states,
    };
    VkGraphicsPipelineCreateInfo pipeline_info = {
        .sType = VK_STRUCTURE_TYPE_GRAPHICS_PIPELINE_CREATE_INFO,
        .stageCount = 2,
        .pStages = stages,
        .pVertexInputState = &vertex_input,
        .pInputAssemblyState = &assembly,
        .pViewportState = &viewport,
        .pRasterizationState = &raster,
        .pMultisampleState = &multisample,
        .pColorBlendState = &blend,
        .pDynamicState = &dynamic,
        .layout = layout,
        .renderPass = render_pass,
    };
    CHECK(vkCreateGraphicsPipelines(dev, VK_NULL_HANDLE, 1, &pipeline_info, NULL, &pipeline));
    say("pipeline (NAK compiled both shaders) in %.1f ms",
        armTicksToNs(armGetSystemTick() - t0) / 1e6);

    VkCommandPoolCreateInfo pool_info = {
        .sType = VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO,
        .flags = VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT,
        .queueFamilyIndex = family,
    };
    CHECK(vkCreateCommandPool(dev, &pool_info, NULL, &pool));
    VkCommandBuffer cmd;
    VkCommandBufferAllocateInfo cmd_info = {
        .sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO,
        .commandPool = pool,
        .level = VK_COMMAND_BUFFER_LEVEL_PRIMARY,
        .commandBufferCount = 1,
    };
    CHECK(vkAllocateCommandBuffers(dev, &cmd_info, &cmd));
    VkSemaphoreCreateInfo sem_info = {.sType = VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO};
    CHECK(vkCreateSemaphore(dev, &sem_info, NULL, &acquired));
    VkFenceCreateInfo fence_info = {
        .sType = VK_STRUCTURE_TYPE_FENCE_CREATE_INFO,
        .flags = VK_FENCE_CREATE_SIGNALED_BIT,
    };
    CHECK(vkCreateFence(dev, &fence_info, NULL, &fence));

    say("drawing for %d s; + exits", RUN_SECONDS);
    double loop_start = seconds(), last_report = loop_start;
    uint32_t frames = 0, frames_since = 0;
    double worst_ms = 0.0;
    while (appletMainLoop()) {
        padUpdate(&pad);
        if (padGetButtonsDown(&pad) & HidNpadButton_Plus) {
            say("+ pressed");
            break;
        }
        double now = seconds();
        if (now - loop_start > RUN_SECONDS)
            break;

        double frame_start = now;
        CHECK(vkWaitForFences(dev, 1, &fence, VK_TRUE, 2000000000ull));
        CHECK(vkResetFences(dev, 1, &fence));
        uint32_t index = 0;
        VkResult acquire =
            vkAcquireNextImageKHR(dev, swapchain, 2000000000ull, acquired, VK_NULL_HANDLE, &index);
        if (acquire != VK_SUCCESS && acquire != VK_SUBOPTIMAL_KHR) {
            say("FAIL vkAcquireNextImageKHR = %d on frame %u", (int)acquire, frames);
            goto fail;
        }

        CHECK(vkResetCommandBuffer(cmd, 0));
        VkCommandBufferBeginInfo begin = {
            .sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO,
            .flags = VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT,
        };
        CHECK(vkBeginCommandBuffer(cmd, &begin));
        float t = (float)(now - loop_start);
        VkClearValue clear = {.color = {{0.1f, 0.1f, 0.25f + 0.15f * sinf(t), 1.0f}}};
        VkRenderPassBeginInfo rp_begin = {
            .sType = VK_STRUCTURE_TYPE_RENDER_PASS_BEGIN_INFO,
            .renderPass = render_pass,
            .framebuffer = framebuffers[index],
            .renderArea = {{0, 0}, extent},
            .clearValueCount = 1,
            .pClearValues = &clear,
        };
        vkCmdBeginRenderPass(cmd, &rp_begin, VK_SUBPASS_CONTENTS_INLINE);
        vkCmdBindPipeline(cmd, VK_PIPELINE_BIND_POINT_GRAPHICS, pipeline);
        VkViewport vp = {0, 0, (float)extent.width, (float)extent.height, 0, 1};
        VkRect2D scissor = {{0, 0}, extent};
        vkCmdSetViewport(cmd, 0, 1, &vp);
        vkCmdSetScissor(cmd, 0, 1, &scissor);
        vkCmdPushConstants(cmd, layout, VK_SHADER_STAGE_VERTEX_BIT, 0, sizeof t, &t);
        vkCmdDraw(cmd, 3, 1, 0, 0);
        vkCmdEndRenderPass(cmd);
        CHECK(vkEndCommandBuffer(cmd));

        VkPipelineStageFlags wait_stage = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
        VkSubmitInfo submit = {
            .sType = VK_STRUCTURE_TYPE_SUBMIT_INFO,
            .waitSemaphoreCount = 1,
            .pWaitSemaphores = &acquired,
            .pWaitDstStageMask = &wait_stage,
            .commandBufferCount = 1,
            .pCommandBuffers = &cmd,
            .signalSemaphoreCount = 1,
            .pSignalSemaphores = &rendered[index],
        };
        CHECK(vkQueueSubmit(queue, 1, &submit, fence));
        VkPresentInfoKHR present = {
            .sType = VK_STRUCTURE_TYPE_PRESENT_INFO_KHR,
            .waitSemaphoreCount = 1,
            .pWaitSemaphores = &rendered[index],
            .swapchainCount = 1,
            .pSwapchains = &swapchain,
            .pImageIndices = &index,
        };
        VkResult pr = vkQueuePresentKHR(queue, &present);
        if (pr != VK_SUCCESS && pr != VK_SUBOPTIMAL_KHR) {
            say("FAIL vkQueuePresentKHR = %d on frame %u", (int)pr, frames);
            goto fail;
        }
        double frame_ms = (seconds() - frame_start) * 1000.0;
        if (frames < 3)
            say("frame %u: image %u, acquire %d, present %d, %.2f ms", frames, index,
                (int)acquire, (int)pr, frame_ms);
        if (frames > 0 && frame_ms > worst_ms)
            worst_ms = frame_ms;
        frames++;
        frames_since++;
        if (seconds() - last_report >= 5.0) {
            say("%.1f fps over the last %.1f s (worst frame %.2f ms)",
                frames_since / (seconds() - last_report), seconds() - last_report, worst_ms);
            last_report = seconds();
            frames_since = 0;
            worst_ms = 0.0;
        }
    }
    say("drew %u frames in %.1f s", frames, seconds() - loop_start);
    ok = 1;

fail:
    if (!ok) {
        say("stopped after a failure; + exits (10 s timeout)");
        double until = seconds() + 10.0;
        while (appletMainLoop() && seconds() < until) {
            padUpdate(&pad);
            if (padGetButtonsDown(&pad) & HidNpadButton_Plus)
                break;
            svcSleepThread(16000000);
        }
    }
    if (dev) {
        vkDeviceWaitIdle(dev);
        if (fence) vkDestroyFence(dev, fence, NULL);
        if (acquired) vkDestroySemaphore(dev, acquired, NULL);
        if (pool) vkDestroyCommandPool(dev, pool, NULL);
        if (pipeline) vkDestroyPipeline(dev, pipeline, NULL);
        if (layout) vkDestroyPipelineLayout(dev, layout, NULL);
        if (vert) vkDestroyShaderModule(dev, vert, NULL);
        if (frag) vkDestroyShaderModule(dev, frag, NULL);
        for (uint32_t i = 0; i < MAX_IMAGES; i++) {
            if (rendered[i]) vkDestroySemaphore(dev, rendered[i], NULL);
            if (framebuffers[i]) vkDestroyFramebuffer(dev, framebuffers[i], NULL);
            if (views[i]) vkDestroyImageView(dev, views[i], NULL);
        }
        if (render_pass) vkDestroyRenderPass(dev, render_pass, NULL);
        if (swapchain) vkDestroySwapchainKHR(dev, swapchain, NULL);
        vkDestroyDevice(dev, NULL);
    }
    if (surface) vkDestroySurfaceKHR(instance, surface, NULL);
    if (messenger) {
        PFN_vkDestroyDebugUtilsMessengerEXT destroy_messenger =
            (PFN_vkDestroyDebugUtilsMessengerEXT)vkGetInstanceProcAddr(
                instance, "vkDestroyDebugUtilsMessengerEXT");
        if (destroy_messenger)
            destroy_messenger(instance, messenger, NULL);
    }
    if (instance) vkDestroyInstance(instance, NULL);
    say("%s exit", ok ? "clean" : "failed");
    if (g_log)
        fclose(g_log);
    return 0;
}
