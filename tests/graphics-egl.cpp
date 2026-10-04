// Host-built Windows fixture: GLES 3 shader readback through EGL, then a
// window.
#define WIN32_LEAN_AND_MEAN
#include <EGL/egl.h>
#include <GLES3/gl3.h>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <stdexcept>
#include <string>
#include <windows.h>

static void require(bool ok, const char *message) {
  if (!ok)
    throw std::runtime_error(message);
}
template <class T> static T symbol(HMODULE library, const char *name) {
  FARPROC raw = GetProcAddress(library, name);
  require(raw, name);
  T typed;
  static_assert(sizeof(typed) == sizeof(raw));
  std::memcpy(&typed, &raw, sizeof(typed));
  return typed;
}
static std::string quoted(const char *text) {
  require(text, "missing renderer string");
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
    require(std::getenv("GALLIUM_DRIVER") &&
                !std::strcmp(std::getenv("GALLIUM_DRIVER"), "llvmpipe"),
            "renderer environment was not inherited");
    HMODULE egl = LoadLibraryA("libEGL.dll"),
            gles = LoadLibraryA("libGLESv2.dll");
    require(egl && gles, "EGL/GLES libraries");
#define EGLFN(type, name) auto name = symbol<type>(egl, #name)
#define GLFN(type, name) auto name = symbol<type>(gles, #name)
    EGLFN(PFNEGLGETDISPLAYPROC, eglGetDisplay);
    EGLFN(PFNEGLINITIALIZEPROC, eglInitialize);
    EGLFN(PFNEGLBINDAPIPROC, eglBindAPI);
    EGLFN(PFNEGLCHOOSECONFIGPROC, eglChooseConfig);
    EGLFN(PFNEGLCREATECONTEXTPROC, eglCreateContext);
    EGLFN(PFNEGLCREATEPBUFFERSURFACEPROC, eglCreatePbufferSurface);
    EGLFN(PFNEGLCREATEWINDOWSURFACEPROC, eglCreateWindowSurface);
    EGLFN(PFNEGLMAKECURRENTPROC, eglMakeCurrent);
    EGLFN(PFNEGLSWAPBUFFERSPROC, eglSwapBuffers);
    EGLFN(PFNEGLDESTROYSURFACEPROC, eglDestroySurface);
    EGLFN(PFNEGLDESTROYCONTEXTPROC, eglDestroyContext);
    EGLFN(PFNEGLTERMINATEPROC, eglTerminate);
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
    GLFN(PFNGLVIEWPORTPROC, glViewport);
    GLFN(PFNGLCLEARCOLORPROC, glClearColor);
    GLFN(PFNGLCLEARPROC, glClear);
    GLFN(PFNGLDRAWARRAYSPROC, glDrawArrays);
    GLFN(PFNGLFINISHPROC, glFinish);
    GLFN(PFNGLREADPIXELSPROC, glReadPixels);
    GLFN(PFNGLGETERRORPROC, glGetError);
    GLFN(PFNGLGETSTRINGPROC, glGetString);
    WNDCLASSA klass{};
    klass.style = CS_OWNDC;
    klass.lpfnWndProc = DefWindowProcA;
    klass.hInstance = GetModuleHandleA(nullptr);
    klass.lpszClassName = "wdesk-egl-probe";
    require(RegisterClassA(&klass), "EGL window class");
    HWND hwnd = CreateWindowA(klass.lpszClassName, "wdesk EGL probe",
                              WS_OVERLAPPEDWINDOW, 80, 80, 256, 256, nullptr,
                              nullptr, klass.hInstance, nullptr);
    require(hwnd, "EGL window");
    HDC dc = GetDC(hwnd);
    EGLDisplay display = eglGetDisplay(dc);
    require(display != EGL_NO_DISPLAY, "EGL display");
    EGLint major = 0, minor = 0;
    require(eglInitialize(display, &major, &minor), "EGL initialize");
    require(eglBindAPI(EGL_OPENGL_ES_API), "EGL bind GLES");
    const EGLint attributes[] = {EGL_SURFACE_TYPE,
                                 EGL_PBUFFER_BIT | EGL_WINDOW_BIT,
                                 EGL_RENDERABLE_TYPE,
                                 EGL_OPENGL_ES3_BIT,
                                 EGL_RED_SIZE,
                                 8,
                                 EGL_GREEN_SIZE,
                                 8,
                                 EGL_BLUE_SIZE,
                                 8,
                                 EGL_ALPHA_SIZE,
                                 8,
                                 EGL_NONE};
    EGLConfig config{};
    EGLint count = 0;
    require(eglChooseConfig(display, attributes, &config, 1, &count) &&
                count == 1,
            "EGL config");
    const EGLint context_attributes[] = {EGL_CONTEXT_CLIENT_VERSION, 3,
                                         EGL_NONE};
    EGLContext context =
        eglCreateContext(display, config, EGL_NO_CONTEXT, context_attributes);
    require(context != EGL_NO_CONTEXT, "GLES3 context");
    const EGLint pbuffer_attributes[] = {EGL_WIDTH, 128, EGL_HEIGHT, 128,
                                         EGL_NONE};
    EGLSurface pbuffer =
        eglCreatePbufferSurface(display, config, pbuffer_attributes);
    require(pbuffer != EGL_NO_SURFACE, "EGL pbuffer");
    require(eglMakeCurrent(display, pbuffer, pbuffer, context),
            "EGL pbuffer current");
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
    const char *vertex = "#version 300 es\nconst vec2 "
                         "v[3]=vec2[3](vec2(-1,-1),vec2(1,-1),vec2(0,1));void "
                         "main(){gl_Position=vec4(v[gl_VertexID],0,1);}";
    const char *fragment = "#version 300 es\nprecision mediump float;out vec4 "
                           "color;void main(){color=vec4(0.25,0.5,0.75,1);}";
    GLuint program = glCreateProgram();
    glAttachShader(program, shader(GL_VERTEX_SHADER, vertex));
    glAttachShader(program, shader(GL_FRAGMENT_SHADER, fragment));
    glLinkProgram(program);
    GLint success = 0;
    glGetProgramiv(program, GL_LINK_STATUS, &success);
    require(success, "GLES shader link");
    glUseProgram(program);
    GLuint vao = 0;
    glGenVertexArrays(1, &vao);
    glBindVertexArray(vao);
    auto draw = [&]() {
      glViewport(0, 0, 128, 128);
      glClearColor(0, 0, 0, 1);
      glClear(GL_COLOR_BUFFER_BIT);
      glDrawArrays(GL_TRIANGLES, 0, 3);
      glFinish();
      unsigned char rgba[4]{};
      glReadPixels(64, 64, 1, 1, GL_RGBA, GL_UNSIGNED_BYTE, rgba);
      require(glGetError() == GL_NO_ERROR, "GLES error");
      const int expected[] = {64, 128, 191, 255};
      for (unsigned i = 0; i < 4; ++i)
        require(std::abs(int(rgba[i]) - expected[i]) <= 1,
                "GLES rendered pixel mismatch");
    };
    draw();
    EGLSurface surface = eglCreateWindowSurface(display, config, hwnd, nullptr);
    require(surface != EGL_NO_SURFACE, "EGL window surface");
    require(eglMakeCurrent(display, surface, surface, context),
            "EGL window current");
    ShowWindow(hwnd, SW_SHOW);
    UpdateWindow(hwnd);
    draw();
    require(eglSwapBuffers(display, surface), "EGL window presentation");
    std::printf("{\"api\":\"gles\",\"egl\":\"%d.%d\",\"architecture\":\"%s\","
                "\"renderer\":%s,\"version\":%s,\"shader\":\"GLSL ES "
                "300\",\"pixel\":[64,128,191,255],\"pbuffer\":true,"
                "\"presented\":true}\n",
                major, minor, sizeof(void *) == 8 ? "x64" : "x86",
                quoted(reinterpret_cast<const char *>(glGetString(GL_RENDERER)))
                    .c_str(),
                quoted(reinterpret_cast<const char *>(glGetString(GL_VERSION)))
                    .c_str());
    std::fflush(stdout);
    Sleep(2000);
    eglMakeCurrent(display, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
    eglDestroySurface(display, surface);
    eglDestroySurface(display, pbuffer);
    eglDestroyContext(display, context);
    eglTerminate(display);
    ReleaseDC(hwnd, dc);
    DestroyWindow(hwnd);
    FreeLibrary(gles);
    FreeLibrary(egl);
    return 0;
  } catch (const std::exception &error) {
    std::fprintf(stderr, "%s\n", error.what());
    return 1;
  }
}
