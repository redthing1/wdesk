// Windows-only rendering checks; cross-compile on the host, execute in the
// guest.
#define WIN32_LEAN_AND_MEAN
#define WIDL_EXPLICIT_AGGREGATE_RETURNS
#include <GL/gl.h>
#include <GL/glext.h>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <d3d10.h>
#include <d3d11.h>
#include <d3d12.h>
#include <d3d9.h>
#include <dxgi1_4.h>
#include <stdexcept>
#include <string>
#include <windows.h>

static void require(bool ok, const char *message) {
  if (!ok)
    throw std::runtime_error(message);
}
static void checked(HRESULT result, const char *message) {
  if (FAILED(result))
    throw std::runtime_error(std::string(message) + ": " +
                             std::to_string(static_cast<unsigned>(result)));
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
static void pixel(const unsigned char *rgba) {
  const int expected[] = {64, 128, 191, 255};
  for (unsigned i = 0; i < 4; ++i)
    require(static_cast<int>(rgba[i]) >= expected[i] - 1 &&
                static_cast<int>(rgba[i]) <= expected[i] + 1,
            "rendered pixel mismatch");
}
template <class T> static T glfn(const char *name) {
  PROC raw = wglGetProcAddress(name);
  auto address = reinterpret_cast<std::uintptr_t>(raw);
  require(address > 3 && address != static_cast<std::uintptr_t>(-1), name);
  T typed;
  static_assert(sizeof(typed) == sizeof(raw));
  std::memcpy(&typed, &raw, sizeof(typed));
  return typed;
}
static HWND window() {
  WNDCLASSA klass{};
  klass.style = CS_OWNDC;
  klass.lpfnWndProc = DefWindowProcA;
  klass.hInstance = GetModuleHandleA(nullptr);
  klass.lpszClassName = "wdesk-graphics-probe";
  require(RegisterClassA(&klass) != 0, "RegisterClass");
  auto hwnd = CreateWindowA(klass.lpszClassName, "wdesk graphics probe",
                            WS_OVERLAPPEDWINDOW, 80, 80, 256, 256, nullptr,
                            nullptr, klass.hInstance, nullptr);
  require(hwnd != nullptr, "CreateWindow");
  ShowWindow(hwnd, SW_SHOW);
  UpdateWindow(hwnd);
  return hwnd;
}
static void opengl() {
  require(std::getenv("GALLIUM_DRIVER") &&
              !std::strcmp(std::getenv("GALLIUM_DRIVER"), "llvmpipe"),
          "renderer environment was not inherited");
  auto hwnd = window();
  auto dc = GetDC(hwnd);
  PIXELFORMATDESCRIPTOR format{};
  format.nSize = sizeof(format);
  format.nVersion = 1;
  format.dwFlags = PFD_DRAW_TO_WINDOW | PFD_SUPPORT_OPENGL | PFD_DOUBLEBUFFER;
  format.iPixelType = PFD_TYPE_RGBA;
  format.cColorBits = 24;
  format.cAlphaBits = 8;
  int index = ChoosePixelFormat(dc, &format);
  require(index != 0 && SetPixelFormat(dc, index, &format),
          "OpenGL pixel format");
  auto context = wglCreateContext(dc);
  require(context && wglMakeCurrent(dc, context), "OpenGL context");
#define GLFN(type, name) auto name = glfn<type>(#name)
  GLFN(PFNGLCREATESHADERPROC, glCreateShader);
  GLFN(PFNGLSHADERSOURCEPROC, glShaderSource);
  GLFN(PFNGLCOMPILESHADERPROC, glCompileShader);
  GLFN(PFNGLGETSHADERIVPROC, glGetShaderiv);
  GLFN(PFNGLGETSHADERINFOLOGPROC, glGetShaderInfoLog);
  GLFN(PFNGLCREATEPROGRAMPROC, glCreateProgram);
  GLFN(PFNGLATTACHSHADERPROC, glAttachShader);
  GLFN(PFNGLLINKPROGRAMPROC, glLinkProgram);
  GLFN(PFNGLGETPROGRAMIVPROC, glGetProgramiv);
  GLFN(PFNGLUSEPROGRAMPROC, glUseProgram);
  GLFN(PFNGLGENVERTEXARRAYSPROC, glGenVertexArrays);
  GLFN(PFNGLBINDVERTEXARRAYPROC, glBindVertexArray);
  const char *vertex = "#version 330\nconst vec2 "
                       "v[3]=vec2[3](vec2(-1,-1),vec2(1,-1),vec2(0,1));void "
                       "main(){gl_Position=vec4(v[gl_VertexID],0,1);}";
  const char *fragment =
      "#version 330\nout vec4 color;void main(){color=vec4(0.25,0.5,0.75,1);}";
  auto shader = [&](GLenum kind, const char *source) {
    GLuint result = glCreateShader(kind);
    glShaderSource(result, 1, &source, nullptr);
    glCompileShader(result);
    GLint success = 0;
    glGetShaderiv(result, GL_COMPILE_STATUS, &success);
    if (!success) {
      char log[1024]{};
      glGetShaderInfoLog(result, sizeof(log), nullptr, log);
      throw std::runtime_error(log);
    }
    return result;
  };
  GLuint program = glCreateProgram();
  glAttachShader(program, shader(GL_VERTEX_SHADER, vertex));
  glAttachShader(program, shader(GL_FRAGMENT_SHADER, fragment));
  glLinkProgram(program);
  GLint success = 0;
  glGetProgramiv(program, GL_LINK_STATUS, &success);
  require(success != 0, "GLSL program link");
  glUseProgram(program);
  GLuint vao = 0;
  glGenVertexArrays(1, &vao);
  glBindVertexArray(vao);
  glViewport(0, 0, 128, 128);
  glClearColor(0, 0, 0, 1);
  glClear(GL_COLOR_BUFFER_BIT);
  glDrawArrays(GL_TRIANGLES, 0, 3);
  glFinish();
  unsigned char rgba[4]{};
  glReadPixels(64, 64, 1, 1, GL_RGBA, GL_UNSIGNED_BYTE, rgba);
  require(glGetError() == GL_NO_ERROR, "OpenGL error");
  pixel(rgba);
  require(SwapBuffers(dc), "OpenGL window presentation");
  std::printf(
      "{\"api\":\"gl\",\"renderer\":%s,\"version\":%s,\"shader\":\"GLSL "
      "330\",\"pixel\":[%u,%u,%u,%u],\"presented\":true}\n",
      quoted(reinterpret_cast<const char *>(glGetString(GL_RENDERER))).c_str(),
      quoted(reinterpret_cast<const char *>(glGetString(GL_VERSION))).c_str(),
      rgba[0], rgba[1], rgba[2], rgba[3]);
  std::fflush(stdout);
  Sleep(2000);
  wglMakeCurrent(nullptr, nullptr);
  wglDeleteContext(context);
  ReleaseDC(hwnd, dc);
  DestroyWindow(hwnd);
}
static void d3d11() {
  ID3D11Device *device = nullptr;
  ID3D11DeviceContext *context = nullptr;
  D3D_FEATURE_LEVEL level{};
  const D3D_FEATURE_LEVEL levels[] = {
      D3D_FEATURE_LEVEL_12_1, D3D_FEATURE_LEVEL_12_0, D3D_FEATURE_LEVEL_11_1,
      D3D_FEATURE_LEVEL_11_0};
  checked(D3D11CreateDevice(nullptr, D3D_DRIVER_TYPE_WARP, nullptr, 0, levels,
                            4, D3D11_SDK_VERSION, &device, &level, &context),
          "D3D11 WARP device");
  D3D11_TEXTURE2D_DESC desc{};
  desc.Width = desc.Height = 64;
  desc.MipLevels = desc.ArraySize = 1;
  desc.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
  desc.SampleDesc.Count = 1;
  desc.Usage = D3D11_USAGE_DEFAULT;
  desc.BindFlags = D3D11_BIND_RENDER_TARGET;
  ID3D11Texture2D *texture = nullptr;
  checked(device->CreateTexture2D(&desc, nullptr, &texture),
          "D3D11 render texture");
  ID3D11RenderTargetView *target = nullptr;
  checked(device->CreateRenderTargetView(texture, nullptr, &target),
          "D3D11 render target");
  const float color[] = {0.25f, 0.5f, 0.75f, 1};
  context->ClearRenderTargetView(target, color);
  desc.Usage = D3D11_USAGE_STAGING;
  desc.BindFlags = 0;
  desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ;
  ID3D11Texture2D *staging = nullptr;
  checked(device->CreateTexture2D(&desc, nullptr, &staging),
          "D3D11 staging texture");
  context->CopyResource(staging, texture);
  D3D11_MAPPED_SUBRESOURCE mapped{};
  checked(context->Map(staging, 0, D3D11_MAP_READ, 0, &mapped),
          "D3D11 pixel readback");
  auto rgba = static_cast<unsigned char *>(mapped.pData) +
              32 * mapped.RowPitch + 32 * 4;
  pixel(rgba);
  std::printf("{\"api\":\"d3d11\",\"renderer\":\"WARP\",\"feature_level\":%u,"
              "\"pixel\":[%u,%u,%u,%u],\"presented\":false}\n",
              static_cast<unsigned>(level), rgba[0], rgba[1], rgba[2], rgba[3]);
  context->Unmap(staging, 0);
  staging->Release();
  target->Release();
  texture->Release();
  context->Release();
  device->Release();
}
static void d3d9() {
  auto hwnd = window();
  IDirect3D9 *api = Direct3DCreate9(D3D_SDK_VERSION);
  require(api, "D3D9 runtime");
  D3DADAPTER_IDENTIFIER9 adapter{};
  checked(api->GetAdapterIdentifier(D3DADAPTER_DEFAULT, 0, &adapter),
          "D3D9 adapter");
  D3DPRESENT_PARAMETERS params{};
  params.BackBufferWidth = params.BackBufferHeight = 64;
  params.BackBufferFormat = D3DFMT_A8R8G8B8;
  params.BackBufferCount = 1;
  params.SwapEffect = D3DSWAPEFFECT_DISCARD;
  params.hDeviceWindow = hwnd;
  params.Windowed = TRUE;
  IDirect3DDevice9 *device = nullptr;
  checked(api->CreateDevice(D3DADAPTER_DEFAULT, D3DDEVTYPE_HAL, hwnd,
                            D3DCREATE_SOFTWARE_VERTEXPROCESSING, &params,
                            &device),
          "D3D9 basic display device");
  checked(device->Clear(0, nullptr, D3DCLEAR_TARGET,
                        D3DCOLOR_ARGB(255, 64, 128, 191), 1, 0),
          "D3D9 clear");
  IDirect3DSurface9 *target = nullptr;
  IDirect3DSurface9 *staging = nullptr;
  checked(device->GetBackBuffer(0, 0, D3DBACKBUFFER_TYPE_MONO, &target),
          "D3D9 back buffer");
  checked(device->CreateOffscreenPlainSurface(
              64, 64, D3DFMT_A8R8G8B8, D3DPOOL_SYSTEMMEM, &staging, nullptr),
          "D3D9 readback surface");
  checked(device->GetRenderTargetData(target, staging), "D3D9 readback");
  D3DLOCKED_RECT mapped{};
  checked(staging->LockRect(&mapped, nullptr, D3DLOCK_READONLY), "D3D9 lock");
  auto bgra =
      static_cast<unsigned char *>(mapped.pBits) + 32 * mapped.Pitch + 32 * 4;
  const unsigned char rgba[] = {bgra[2], bgra[1], bgra[0], bgra[3]};
  pixel(rgba);
  checked(staging->UnlockRect(), "D3D9 unlock");
  checked(device->Present(nullptr, nullptr, nullptr, nullptr),
          "D3D9 presentation");
  std::printf("{\"api\":\"d3d9\",\"renderer\":%s,\"driver\":%s,\"pixel\":[%u,%"
              "u,%u,%u],\"presented\":true}\n",
              quoted(adapter.Description).c_str(),
              quoted(adapter.Driver).c_str(), rgba[0], rgba[1], rgba[2],
              rgba[3]);
  std::fflush(stdout);
  Sleep(2000);
  staging->Release();
  target->Release();
  device->Release();
  api->Release();
  DestroyWindow(hwnd);
}
static void d3d10() {
  ID3D10Device *device = nullptr;
  checked(D3D10CreateDevice(nullptr, D3D10_DRIVER_TYPE_WARP, nullptr, 0,
                            D3D10_SDK_VERSION, &device),
          "D3D10 WARP device");
  D3D10_TEXTURE2D_DESC desc{};
  desc.Width = desc.Height = 64;
  desc.MipLevels = desc.ArraySize = 1;
  desc.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
  desc.SampleDesc.Count = 1;
  desc.Usage = D3D10_USAGE_DEFAULT;
  desc.BindFlags = D3D10_BIND_RENDER_TARGET;
  ID3D10Texture2D *texture = nullptr;
  ID3D10RenderTargetView *target = nullptr;
  checked(device->CreateTexture2D(&desc, nullptr, &texture),
          "D3D10 render texture");
  checked(device->CreateRenderTargetView(texture, nullptr, &target),
          "D3D10 render target");
  const float color[] = {0.25f, 0.5f, 0.75f, 1};
  device->ClearRenderTargetView(target, color);
  desc.Usage = D3D10_USAGE_STAGING;
  desc.BindFlags = 0;
  desc.CPUAccessFlags = D3D10_CPU_ACCESS_READ;
  ID3D10Texture2D *staging = nullptr;
  checked(device->CreateTexture2D(&desc, nullptr, &staging),
          "D3D10 staging texture");
  device->CopyResource(staging, texture);
  D3D10_MAPPED_TEXTURE2D mapped{};
  checked(staging->Map(0, D3D10_MAP_READ, 0, &mapped), "D3D10 pixel readback");
  auto rgba = static_cast<unsigned char *>(mapped.pData) +
              32 * mapped.RowPitch + 32 * 4;
  pixel(rgba);
  std::printf("{\"api\":\"d3d10\",\"renderer\":\"WARP\",\"feature_level\":"
              "40960,\"pixel\":[%u,%u,%u,%u],\"presented\":false}\n",
              rgba[0], rgba[1], rgba[2], rgba[3]);
  staging->Unmap(0);
  staging->Release();
  target->Release();
  texture->Release();
  device->Release();
}
static void d3d12() {
  IDXGIFactory4 *factory = nullptr;
  IDXGIAdapter1 *adapter = nullptr;
  ID3D12Device *device = nullptr;
  checked(CreateDXGIFactory1(IID_PPV_ARGS(&factory)), "DXGI factory");
  checked(factory->EnumWarpAdapter(IID_PPV_ARGS(&adapter)),
          "D3D12 WARP adapter");
  DXGI_ADAPTER_DESC1 identity{};
  checked(adapter->GetDesc1(&identity), "D3D12 adapter identity");
  require((identity.Flags & DXGI_ADAPTER_FLAG_SOFTWARE) != 0,
          "D3D12 adapter is not software");
  checked(
      D3D12CreateDevice(adapter, D3D_FEATURE_LEVEL_11_0, IID_PPV_ARGS(&device)),
      "D3D12 WARP device");
  const D3D_FEATURE_LEVEL levels[] = {
      static_cast<D3D_FEATURE_LEVEL>(0xc200), D3D_FEATURE_LEVEL_12_1,
      D3D_FEATURE_LEVEL_12_0, D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0};
  D3D12_FEATURE_DATA_FEATURE_LEVELS features{5, levels, D3D_FEATURE_LEVEL_11_0};
  checked(device->CheckFeatureSupport(D3D12_FEATURE_FEATURE_LEVELS, &features,
                                      sizeof(features)),
          "D3D12 feature levels");
  D3D12_COMMAND_QUEUE_DESC queue_desc{};
  queue_desc.Type = D3D12_COMMAND_LIST_TYPE_DIRECT;
  ID3D12CommandQueue *queue = nullptr;
  ID3D12CommandAllocator *allocator = nullptr;
  ID3D12GraphicsCommandList *list = nullptr;
  checked(device->CreateCommandQueue(&queue_desc, IID_PPV_ARGS(&queue)),
          "D3D12 queue");
  checked(device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT,
                                         IID_PPV_ARGS(&allocator)),
          "D3D12 allocator");
  checked(device->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT,
                                    allocator, nullptr, IID_PPV_ARGS(&list)),
          "D3D12 commands");
  D3D12_HEAP_PROPERTIES heap{};
  heap.Type = D3D12_HEAP_TYPE_DEFAULT;
  D3D12_RESOURCE_DESC desc{};
  desc.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
  desc.Width = desc.Height = 64;
  desc.DepthOrArraySize = desc.MipLevels = 1;
  desc.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
  desc.SampleDesc.Count = 1;
  desc.Flags = D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET;
  ID3D12Resource *texture = nullptr;
  ID3D12Resource *staging = nullptr;
  checked(device->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc,
                                          D3D12_RESOURCE_STATE_RENDER_TARGET,
                                          nullptr, IID_PPV_ARGS(&texture)),
          "D3D12 render texture");
  D3D12_PLACED_SUBRESOURCE_FOOTPRINT footprint{};
  UINT64 bytes = 0;
  device->GetCopyableFootprints(&desc, 0, 1, 0, &footprint, nullptr, nullptr,
                                &bytes);
  D3D12_RESOURCE_DESC buffer{};
  buffer.Dimension = D3D12_RESOURCE_DIMENSION_BUFFER;
  buffer.Width = bytes;
  buffer.Height = buffer.DepthOrArraySize = buffer.MipLevels = 1;
  buffer.SampleDesc.Count = 1;
  buffer.Layout = D3D12_TEXTURE_LAYOUT_ROW_MAJOR;
  heap.Type = D3D12_HEAP_TYPE_READBACK;
  checked(device->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &buffer,
                                          D3D12_RESOURCE_STATE_COPY_DEST,
                                          nullptr, IID_PPV_ARGS(&staging)),
          "D3D12 readback buffer");
  D3D12_DESCRIPTOR_HEAP_DESC rtv_desc{};
  rtv_desc.Type = D3D12_DESCRIPTOR_HEAP_TYPE_RTV;
  rtv_desc.NumDescriptors = 1;
  ID3D12DescriptorHeap *rtv = nullptr;
  checked(device->CreateDescriptorHeap(&rtv_desc, IID_PPV_ARGS(&rtv)),
          "D3D12 render target heap");
  auto handle = rtv->GetCPUDescriptorHandleForHeapStart();
  device->CreateRenderTargetView(texture, nullptr, handle);
  const float color[] = {0.25f, 0.5f, 0.75f, 1};
  list->ClearRenderTargetView(handle, color, 0, nullptr);
  D3D12_RESOURCE_BARRIER barrier{};
  barrier.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
  barrier.Transition.pResource = texture;
  barrier.Transition.Subresource = D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES;
  barrier.Transition.StateBefore = D3D12_RESOURCE_STATE_RENDER_TARGET;
  barrier.Transition.StateAfter = D3D12_RESOURCE_STATE_COPY_SOURCE;
  list->ResourceBarrier(1, &barrier);
  D3D12_TEXTURE_COPY_LOCATION from{};
  from.pResource = texture;
  from.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
  D3D12_TEXTURE_COPY_LOCATION to{};
  to.pResource = staging;
  to.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
  to.PlacedFootprint = footprint;
  list->CopyTextureRegion(&to, 0, 0, 0, &from, nullptr);
  checked(list->Close(), "D3D12 close commands");
  ID3D12CommandList *lists[] = {list};
  queue->ExecuteCommandLists(1, lists);
  ID3D12Fence *fence = nullptr;
  checked(device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&fence)),
          "D3D12 fence");
  HANDLE event = CreateEventA(nullptr, FALSE, FALSE, nullptr);
  require(event, "D3D12 fence event");
  checked(fence->SetEventOnCompletion(1, event), "D3D12 completion event");
  checked(queue->Signal(fence, 1), "D3D12 submit fence");
  require(WaitForSingleObject(event, 15000) == WAIT_OBJECT_0,
          "D3D12 completion deadline");
  D3D12_RANGE range{0, static_cast<SIZE_T>(bytes)};
  void *mapped = nullptr;
  checked(staging->Map(0, &range, &mapped), "D3D12 pixel readback");
  auto rgba = static_cast<unsigned char *>(mapped) + footprint.Offset +
              32 * footprint.Footprint.RowPitch + 32 * 4;
  pixel(rgba);
  std::printf("{\"api\":\"d3d12\",\"renderer\":\"WARP\",\"feature_level\":%u,"
              "\"pixel\":[%u,%u,%u,%u],\"presented\":false}\n",
              static_cast<unsigned>(features.MaxSupportedFeatureLevel), rgba[0],
              rgba[1], rgba[2], rgba[3]);
  D3D12_RANGE written{0, 0};
  staging->Unmap(0, &written);
  CloseHandle(event);
  fence->Release();
  rtv->Release();
  staging->Release();
  texture->Release();
  list->Release();
  allocator->Release();
  queue->Release();
  device->Release();
  adapter->Release();
  factory->Release();
}
int main(int argc, char **argv) {
  try {
    require(argc == 2, "Expected gl, d3d9, d3d10, d3d11 or d3d12");
    if (!std::strcmp(argv[1], "gl"))
      opengl();
    else if (!std::strcmp(argv[1], "d3d9"))
      d3d9();
    else if (!std::strcmp(argv[1], "d3d10"))
      d3d10();
    else if (!std::strcmp(argv[1], "d3d11"))
      d3d11();
    else if (!std::strcmp(argv[1], "d3d12"))
      d3d12();
    else
      throw std::runtime_error("Unknown graphics probe");
    return 0;
  } catch (const std::exception &error) {
    std::fprintf(stderr, "%s\n", error.what());
    return 1;
  }
}
