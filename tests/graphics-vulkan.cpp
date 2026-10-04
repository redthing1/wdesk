// Host-built Windows fixture: CPU Vulkan clear/readback and Win32 presentation.
#define WIN32_LEAN_AND_MEAN
#define VK_NO_PROTOTYPES
#define VK_USE_PLATFORM_WIN32_KHR
#include <algorithm>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <stdexcept>
#include <string>
#include <vector>
#include <vulkan/vulkan.h>
#include <windows.h>

static void require(bool ok, const char *message) {
  if (!ok)
    throw std::runtime_error(message);
}
static void checked(VkResult value, const char *message) {
  if (value != VK_SUCCESS && value != VK_SUBOPTIMAL_KHR)
    throw std::runtime_error(std::string(message) + ": " +
                             std::to_string(value));
}
template <class T, class P> static T pointer(P raw, const char *name) {
  require(raw != nullptr, name);
  T typed;
  static_assert(sizeof(typed) == sizeof(raw));
  std::memcpy(&typed, &raw, sizeof(typed));
  return typed;
}
template <class T> static T structure(VkStructureType type) {
  T value{};
  value.sType = type;
  return value;
}
static std::string quoted(const char *text) {
  std::string out = "\"";
  for (; *text; ++text) {
    if (*text == '\\' || *text == '"')
      out += '\\';
    if (static_cast<unsigned char>(*text) >= 32)
      out += *text;
  }
  return out + '"';
}
int main() {
  try {
    require(std::getenv("VK_DRIVER_FILES") && *std::getenv("VK_DRIVER_FILES"),
            "Vulkan driver environment was not inherited");
    HMODULE library = LoadLibraryA("vulkan-1.dll");
    require(library, "Vulkan loader");
    auto get = pointer<PFN_vkGetInstanceProcAddr>(
        GetProcAddress(library, "vkGetInstanceProcAddr"),
        "vkGetInstanceProcAddr");
    auto vkCreateInstance = pointer<PFN_vkCreateInstance>(
        get(nullptr, "vkCreateInstance"), "vkCreateInstance");
    const char *instance_extensions[] = {
        VK_KHR_SURFACE_EXTENSION_NAME, VK_KHR_WIN32_SURFACE_EXTENSION_NAME,
        VK_KHR_GET_SURFACE_CAPABILITIES_2_EXTENSION_NAME,
        VK_EXT_SURFACE_MAINTENANCE_1_EXTENSION_NAME};
    auto app = structure<VkApplicationInfo>(VK_STRUCTURE_TYPE_APPLICATION_INFO);
    app.pApplicationName = "wdesk probe";
    app.apiVersion = VK_API_VERSION_1_1;
    auto instance_info =
        structure<VkInstanceCreateInfo>(VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO);
    instance_info.pApplicationInfo = &app;
    instance_info.enabledExtensionCount = 4;
    instance_info.ppEnabledExtensionNames = instance_extensions;
    VkInstance instance{};
    checked(vkCreateInstance(&instance_info, nullptr, &instance),
            "Vulkan instance");
#define IFN(name) auto name = pointer<PFN_##name>(get(instance, #name), #name)
    IFN(vkEnumeratePhysicalDevices);
    IFN(vkGetPhysicalDeviceProperties);
    IFN(vkGetPhysicalDeviceMemoryProperties);
    IFN(vkGetPhysicalDeviceFeatures2);
    IFN(vkGetPhysicalDeviceQueueFamilyProperties);
    IFN(vkCreateWin32SurfaceKHR);
    IFN(vkGetPhysicalDeviceSurfaceSupportKHR);
    IFN(vkGetPhysicalDeviceSurfaceCapabilitiesKHR);
    IFN(vkGetPhysicalDeviceSurfaceFormatsKHR);
    IFN(vkCreateDevice);
    IFN(vkGetDeviceProcAddr);
    IFN(vkDestroySurfaceKHR);
    IFN(vkDestroyInstance);
    uint32_t count = 0;
    checked(vkEnumeratePhysicalDevices(instance, &count, nullptr),
            "Vulkan device count");
    require(count > 0 && count <= 32, "Vulkan device count bound");
    std::vector<VkPhysicalDevice> devices(count);
    checked(vkEnumeratePhysicalDevices(instance, &count, devices.data()),
            "Vulkan devices");
    VkPhysicalDevice physical{};
    VkPhysicalDeviceProperties properties{};
    for (auto candidate : devices) {
      vkGetPhysicalDeviceProperties(candidate, &properties);
      if (properties.deviceType == VK_PHYSICAL_DEVICE_TYPE_CPU) {
        physical = candidate;
        break;
      }
    }
    require(physical, "No CPU Vulkan device");
    WNDCLASSA klass{};
    klass.lpfnWndProc = DefWindowProcA;
    klass.hInstance = GetModuleHandleA(nullptr);
    klass.lpszClassName = "wdesk-vulkan-probe";
    require(RegisterClassA(&klass), "Vulkan window class");
    HWND hwnd = CreateWindowA(klass.lpszClassName, "wdesk Vulkan probe",
                              WS_OVERLAPPEDWINDOW, 80, 80, 256, 256, nullptr,
                              nullptr, klass.hInstance, nullptr);
    require(hwnd, "Vulkan window");
    auto surface_info = structure<VkWin32SurfaceCreateInfoKHR>(
        VK_STRUCTURE_TYPE_WIN32_SURFACE_CREATE_INFO_KHR);
    surface_info.hinstance = klass.hInstance;
    surface_info.hwnd = hwnd;
    VkSurfaceKHR surface{};
    checked(vkCreateWin32SurfaceKHR(instance, &surface_info, nullptr, &surface),
            "Vulkan Win32 surface");
    vkGetPhysicalDeviceQueueFamilyProperties(physical, &count, nullptr);
    require(count > 0 && count <= 64, "Vulkan queue count bound");
    std::vector<VkQueueFamilyProperties> families(count);
    vkGetPhysicalDeviceQueueFamilyProperties(physical, &count, families.data());
    uint32_t family = count;
    for (uint32_t i = 0; i < count; ++i) {
      VkBool32 supports = VK_FALSE;
      checked(
          vkGetPhysicalDeviceSurfaceSupportKHR(physical, i, surface, &supports),
          "Vulkan surface support");
      if (supports && (families[i].queueFlags & VK_QUEUE_GRAPHICS_BIT)) {
        family = i;
        break;
      }
    }
    require(family < count, "No CPU graphics/presentation queue");
    const float priority = 1;
    auto queue_info = structure<VkDeviceQueueCreateInfo>(
        VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO);
    queue_info.queueFamilyIndex = family;
    queue_info.queueCount = 1;
    queue_info.pQueuePriorities = &priority;
    auto maintenance = structure<
        VkPhysicalDeviceSwapchainMaintenance1FeaturesEXT>(
        VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_SWAPCHAIN_MAINTENANCE_1_FEATURES_EXT);
    auto supported = structure<VkPhysicalDeviceFeatures2>(
        VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_FEATURES_2);
    supported.pNext = &maintenance;
    vkGetPhysicalDeviceFeatures2(physical, &supported);
    require(maintenance.swapchainMaintenance1,
            "Vulkan presentation fence support");
    const char *device_extensions[] = {
        VK_KHR_SWAPCHAIN_EXTENSION_NAME,
        VK_EXT_SWAPCHAIN_MAINTENANCE_1_EXTENSION_NAME};
    auto device_info =
        structure<VkDeviceCreateInfo>(VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO);
    device_info.queueCreateInfoCount = 1;
    device_info.pQueueCreateInfos = &queue_info;
    device_info.pNext = &maintenance;
    device_info.enabledExtensionCount = 2;
    device_info.ppEnabledExtensionNames = device_extensions;
    VkDevice device{};
    checked(vkCreateDevice(physical, &device_info, nullptr, &device),
            "CPU Vulkan device");
#define DFN(name)                                                              \
  auto name = pointer<PFN_##name>(vkGetDeviceProcAddr(device, #name), #name)
    DFN(vkGetDeviceQueue);
    DFN(vkCreateImage);
    DFN(vkGetImageMemoryRequirements);
    DFN(vkAllocateMemory);
    DFN(vkBindImageMemory);
    DFN(vkCreateBuffer);
    DFN(vkGetBufferMemoryRequirements);
    DFN(vkBindBufferMemory);
    DFN(vkCreateCommandPool);
    DFN(vkAllocateCommandBuffers);
    DFN(vkBeginCommandBuffer);
    DFN(vkCmdPipelineBarrier);
    DFN(vkCmdClearColorImage);
    DFN(vkCmdCopyImageToBuffer);
    DFN(vkEndCommandBuffer);
    DFN(vkQueueSubmit);
    DFN(vkCreateFence);
    DFN(vkWaitForFences);
    DFN(vkMapMemory);
    DFN(vkUnmapMemory);
    DFN(vkResetCommandBuffer);
    DFN(vkResetFences);
    DFN(vkCreateSwapchainKHR);
    DFN(vkGetSwapchainImagesKHR);
    DFN(vkCreateSemaphore);
    DFN(vkAcquireNextImageKHR);
    DFN(vkQueuePresentKHR);
    DFN(vkQueueWaitIdle);
    DFN(vkDestroySemaphore);
    DFN(vkDestroySwapchainKHR);
    DFN(vkDestroyFence);
    DFN(vkDestroyCommandPool);
    DFN(vkDestroyBuffer);
    DFN(vkDestroyImage);
    DFN(vkFreeMemory);
    DFN(vkDestroyDevice);
    VkQueue queue{};
    vkGetDeviceQueue(device, family, 0, &queue);
    VkPhysicalDeviceMemoryProperties memory{};
    vkGetPhysicalDeviceMemoryProperties(physical, &memory);
    auto allocate = [&](VkMemoryRequirements needs,
                        VkMemoryPropertyFlags flags) {
      auto info = structure<VkMemoryAllocateInfo>(
          VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO);
      info.allocationSize = needs.size;
      info.memoryTypeIndex = memory.memoryTypeCount;
      for (uint32_t i = 0; i < memory.memoryTypeCount; ++i)
        if ((needs.memoryTypeBits & (1u << i)) &&
            (memory.memoryTypes[i].propertyFlags & flags) == flags) {
          info.memoryTypeIndex = i;
          break;
        }
      require(info.memoryTypeIndex < memory.memoryTypeCount,
              "Vulkan memory type");
      VkDeviceMemory result{};
      checked(vkAllocateMemory(device, &info, nullptr, &result),
              "Vulkan memory allocation");
      return result;
    };
    auto image_info =
        structure<VkImageCreateInfo>(VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO);
    image_info.imageType = VK_IMAGE_TYPE_2D;
    image_info.format = VK_FORMAT_R8G8B8A8_UNORM;
    image_info.extent = {64, 64, 1};
    image_info.mipLevels = image_info.arrayLayers = 1;
    image_info.samples = VK_SAMPLE_COUNT_1_BIT;
    image_info.tiling = VK_IMAGE_TILING_OPTIMAL;
    image_info.usage =
        VK_IMAGE_USAGE_TRANSFER_SRC_BIT | VK_IMAGE_USAGE_TRANSFER_DST_BIT;
    VkImage image{};
    checked(vkCreateImage(device, &image_info, nullptr, &image),
            "Vulkan render image");
    VkMemoryRequirements needs{};
    vkGetImageMemoryRequirements(device, image, &needs);
    auto image_memory = allocate(needs, VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT);
    checked(vkBindImageMemory(device, image, image_memory, 0),
            "Vulkan image memory");
    auto buffer_info =
        structure<VkBufferCreateInfo>(VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO);
    buffer_info.size = 64 * 64 * 4;
    buffer_info.usage = VK_BUFFER_USAGE_TRANSFER_DST_BIT;
    VkBuffer buffer{};
    checked(vkCreateBuffer(device, &buffer_info, nullptr, &buffer),
            "Vulkan readback buffer");
    vkGetBufferMemoryRequirements(device, buffer, &needs);
    auto buffer_memory =
        allocate(needs, VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT |
                            VK_MEMORY_PROPERTY_HOST_COHERENT_BIT);
    checked(vkBindBufferMemory(device, buffer, buffer_memory, 0),
            "Vulkan buffer memory");
    auto pool_info = structure<VkCommandPoolCreateInfo>(
        VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO);
    pool_info.queueFamilyIndex = family;
    pool_info.flags = VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT;
    VkCommandPool pool{};
    checked(vkCreateCommandPool(device, &pool_info, nullptr, &pool),
            "Vulkan command pool");
    auto command_info = structure<VkCommandBufferAllocateInfo>(
        VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO);
    command_info.commandPool = pool;
    command_info.level = VK_COMMAND_BUFFER_LEVEL_PRIMARY;
    command_info.commandBufferCount = 1;
    VkCommandBuffer command{};
    checked(vkAllocateCommandBuffers(device, &command_info, &command),
            "Vulkan command buffer");
    auto begin = structure<VkCommandBufferBeginInfo>(
        VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO);
    begin.flags = VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT;
    checked(vkBeginCommandBuffer(command, &begin), "Vulkan begin commands");
    VkImageSubresourceRange range{VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1};
    auto transition = [&](VkImage target, VkImageLayout before,
                          VkImageLayout after, VkAccessFlags source,
                          VkAccessFlags destination, VkPipelineStageFlags first,
                          VkPipelineStageFlags last) {
      auto barrier = structure<VkImageMemoryBarrier>(
          VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER);
      barrier.srcAccessMask = source;
      barrier.dstAccessMask = destination;
      barrier.oldLayout = before;
      barrier.newLayout = after;
      barrier.srcQueueFamilyIndex = barrier.dstQueueFamilyIndex =
          VK_QUEUE_FAMILY_IGNORED;
      barrier.image = target;
      barrier.subresourceRange = range;
      vkCmdPipelineBarrier(command, first, last, 0, 0, nullptr, 0, nullptr, 1,
                           &barrier);
    };
    const VkClearColorValue color{{0.25f, 0.5f, 0.75f, 1}};
    transition(image, VK_IMAGE_LAYOUT_UNDEFINED,
               VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, 0,
               VK_ACCESS_TRANSFER_WRITE_BIT, VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT,
               VK_PIPELINE_STAGE_TRANSFER_BIT);
    vkCmdClearColorImage(command, image, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
                         &color, 1, &range);
    transition(image, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
               VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
               VK_ACCESS_TRANSFER_WRITE_BIT, VK_ACCESS_TRANSFER_READ_BIT,
               VK_PIPELINE_STAGE_TRANSFER_BIT, VK_PIPELINE_STAGE_TRANSFER_BIT);
    VkBufferImageCopy copy{};
    copy.imageSubresource = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 0, 1};
    copy.imageExtent = {64, 64, 1};
    vkCmdCopyImageToBuffer(command, image, VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
                           buffer, 1, &copy);
    auto host = structure<VkBufferMemoryBarrier>(
        VK_STRUCTURE_TYPE_BUFFER_MEMORY_BARRIER);
    host.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;
    host.dstAccessMask = VK_ACCESS_HOST_READ_BIT;
    host.srcQueueFamilyIndex = host.dstQueueFamilyIndex =
        VK_QUEUE_FAMILY_IGNORED;
    host.buffer = buffer;
    host.size = VK_WHOLE_SIZE;
    vkCmdPipelineBarrier(command, VK_PIPELINE_STAGE_TRANSFER_BIT,
                         VK_PIPELINE_STAGE_HOST_BIT, 0, 0, nullptr, 1, &host, 0,
                         nullptr);
    checked(vkEndCommandBuffer(command), "Vulkan end commands");
    auto fence_info =
        structure<VkFenceCreateInfo>(VK_STRUCTURE_TYPE_FENCE_CREATE_INFO);
    VkFence fence{};
    checked(vkCreateFence(device, &fence_info, nullptr, &fence),
            "Vulkan fence");
    auto submit = structure<VkSubmitInfo>(VK_STRUCTURE_TYPE_SUBMIT_INFO);
    submit.commandBufferCount = 1;
    submit.pCommandBuffers = &command;
    checked(vkQueueSubmit(queue, 1, &submit, fence), "Vulkan submit clear");
    checked(vkWaitForFences(device, 1, &fence, VK_TRUE, 15000000000ull),
            "Vulkan readback deadline");
    void *mapped = nullptr;
    checked(vkMapMemory(device, buffer_memory, 0, VK_WHOLE_SIZE, 0, &mapped),
            "Vulkan mapped readback");
    auto rgba = static_cast<unsigned char *>(mapped) + 32 * 64 * 4 + 32 * 4;
    const int expected[] = {64, 128, 191, 255};
    for (unsigned i = 0; i < 4; ++i)
      require(std::abs(int(rgba[i]) - expected[i]) <= 1,
              "Vulkan rendered pixel mismatch");
    vkUnmapMemory(device, buffer_memory);
    VkSurfaceCapabilitiesKHR caps{};
    checked(vkGetPhysicalDeviceSurfaceCapabilitiesKHR(physical, surface, &caps),
            "Vulkan surface capabilities");
    require(caps.supportedUsageFlags & VK_IMAGE_USAGE_TRANSFER_DST_BIT,
            "Vulkan surface cannot receive rendered pixels");
    checked(vkGetPhysicalDeviceSurfaceFormatsKHR(physical, surface, &count,
                                                 nullptr),
            "Vulkan surface format count");
    require(count > 0 && count <= 64, "Vulkan surface format bound");
    std::vector<VkSurfaceFormatKHR> formats(count);
    checked(vkGetPhysicalDeviceSurfaceFormatsKHR(physical, surface, &count,
                                                 formats.data()),
            "Vulkan surface formats");
    auto format = formats.front();
    for (auto candidate : formats)
      if (candidate.format == VK_FORMAT_B8G8R8A8_UNORM)
        format = candidate;
    auto swap_info = structure<VkSwapchainCreateInfoKHR>(
        VK_STRUCTURE_TYPE_SWAPCHAIN_CREATE_INFO_KHR);
    swap_info.surface = surface;
    swap_info.minImageCount = std::max(2u, caps.minImageCount);
    if (caps.maxImageCount)
      swap_info.minImageCount =
          std::min(swap_info.minImageCount, caps.maxImageCount);
    swap_info.imageFormat = format.format;
    swap_info.imageColorSpace = format.colorSpace;
    swap_info.imageExtent = caps.currentExtent;
    if (swap_info.imageExtent.width == UINT32_MAX)
      swap_info.imageExtent = {std::clamp(128u, caps.minImageExtent.width,
                                          caps.maxImageExtent.width),
                               std::clamp(128u, caps.minImageExtent.height,
                                          caps.maxImageExtent.height)};
    swap_info.imageArrayLayers = 1;
    swap_info.imageUsage = VK_IMAGE_USAGE_TRANSFER_DST_BIT;
    swap_info.preTransform = caps.currentTransform;
    swap_info.compositeAlpha = VK_COMPOSITE_ALPHA_OPAQUE_BIT_KHR;
    if (!(caps.supportedCompositeAlpha & swap_info.compositeAlpha))
      swap_info.compositeAlpha = static_cast<VkCompositeAlphaFlagBitsKHR>(
          caps.supportedCompositeAlpha & (~caps.supportedCompositeAlpha + 1));
    swap_info.presentMode = VK_PRESENT_MODE_FIFO_KHR;
    swap_info.clipped = VK_TRUE;
    VkSwapchainKHR swap{};
    checked(vkCreateSwapchainKHR(device, &swap_info, nullptr, &swap),
            "Vulkan swapchain");
    checked(vkGetSwapchainImagesKHR(device, swap, &count, nullptr),
            "Vulkan swap image count");
    require(count > 0 && count <= 16, "Vulkan swap image bound");
    std::vector<VkImage> images(count);
    checked(vkGetSwapchainImagesKHR(device, swap, &count, images.data()),
            "Vulkan swap images");
    auto semaphore_info = structure<VkSemaphoreCreateInfo>(
        VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO);
    VkSemaphore acquired{}, complete{};
    checked(vkCreateSemaphore(device, &semaphore_info, nullptr, &acquired),
            "Vulkan acquire semaphore");
    checked(vkCreateSemaphore(device, &semaphore_info, nullptr, &complete),
            "Vulkan render semaphore");
    ShowWindow(hwnd, SW_SHOW);
    UpdateWindow(hwnd);
    uint32_t index = 0;
    checked(vkAcquireNextImageKHR(device, swap, 5000000000ull, acquired,
                                  VK_NULL_HANDLE, &index),
            "Vulkan acquire window image");
    checked(vkResetFences(device, 1, &fence), "Vulkan reset fence");
    checked(vkResetCommandBuffer(command, 0), "Vulkan reset commands");
    checked(vkBeginCommandBuffer(command, &begin), "Vulkan window commands");
    transition(images[index], VK_IMAGE_LAYOUT_UNDEFINED,
               VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, 0,
               VK_ACCESS_TRANSFER_WRITE_BIT, VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT,
               VK_PIPELINE_STAGE_TRANSFER_BIT);
    vkCmdClearColorImage(command, images[index],
                         VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, &color, 1,
                         &range);
    transition(images[index], VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
               VK_IMAGE_LAYOUT_PRESENT_SRC_KHR, VK_ACCESS_TRANSFER_WRITE_BIT, 0,
               VK_PIPELINE_STAGE_TRANSFER_BIT,
               VK_PIPELINE_STAGE_BOTTOM_OF_PIPE_BIT);
    checked(vkEndCommandBuffer(command), "Vulkan window commands end");
    VkPipelineStageFlags wait_stage = VK_PIPELINE_STAGE_TRANSFER_BIT;
    submit.waitSemaphoreCount = 1;
    submit.pWaitSemaphores = &acquired;
    submit.pWaitDstStageMask = &wait_stage;
    submit.signalSemaphoreCount = 1;
    submit.pSignalSemaphores = &complete;
    checked(vkQueueSubmit(queue, 1, &submit, fence), "Vulkan window submit");
    auto present =
        structure<VkPresentInfoKHR>(VK_STRUCTURE_TYPE_PRESENT_INFO_KHR);
    present.waitSemaphoreCount = 1;
    present.pWaitSemaphores = &complete;
    present.swapchainCount = 1;
    present.pSwapchains = &swap;
    present.pImageIndices = &index;
    VkFence presented{};
    checked(vkCreateFence(device, &fence_info, nullptr, &presented),
            "Vulkan presentation fence");
    auto completion = structure<VkSwapchainPresentFenceInfoEXT>(
        VK_STRUCTURE_TYPE_SWAPCHAIN_PRESENT_FENCE_INFO_EXT);
    completion.swapchainCount = 1;
    completion.pFences = &presented;
    present.pNext = &completion;
    checked(vkQueuePresentKHR(queue, &present), "Vulkan presentation");
    // Queue idle alone does not prove presentation resources can be destroyed.
    checked(vkWaitForFences(device, 1, &presented, VK_TRUE, 15000000000ull),
            "Vulkan presentation deadline");
    checked(vkQueueWaitIdle(queue), "Vulkan window completion");
    std::printf("{\"api\":\"vk\",\"architecture\":\"%s\",\"renderer\":%s,"
                "\"device_type\":\"CPU\",\"version\":\"%u.%u.%u\",\"pixel\":["
                "64,128,191,255],\"presented\":true}\n",
                sizeof(void *) == 8 ? "x64" : "x86",
                quoted(properties.deviceName).c_str(),
                VK_API_VERSION_MAJOR(properties.apiVersion),
                VK_API_VERSION_MINOR(properties.apiVersion),
                VK_API_VERSION_PATCH(properties.apiVersion));
    std::fflush(stdout);
    Sleep(2000);
    vkDestroySemaphore(device, complete, nullptr);
    vkDestroySemaphore(device, acquired, nullptr);
    vkDestroySwapchainKHR(device, swap, nullptr);
    vkDestroyFence(device, presented, nullptr);
    vkDestroyFence(device, fence, nullptr);
    vkDestroyCommandPool(device, pool, nullptr);
    vkDestroyBuffer(device, buffer, nullptr);
    vkDestroyImage(device, image, nullptr);
    vkFreeMemory(device, buffer_memory, nullptr);
    vkFreeMemory(device, image_memory, nullptr);
    vkDestroyDevice(device, nullptr);
    vkDestroySurfaceKHR(instance, surface, nullptr);
    vkDestroyInstance(instance, nullptr);
    DestroyWindow(hwnd);
    FreeLibrary(library);
    return 0;
  } catch (const std::exception &error) {
    std::fprintf(stderr, "%s\n", error.what());
    return 1;
  }
}
