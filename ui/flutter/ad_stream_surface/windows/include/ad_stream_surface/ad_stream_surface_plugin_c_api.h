#ifndef AD_STREAM_SURFACE_PLUGIN_C_API_H_
#define AD_STREAM_SURFACE_PLUGIN_C_API_H_
#include <flutter_plugin_registrar.h>
#ifdef FLUTTER_PLUGIN_IMPL
#define AD_STREAM_EXPORT __declspec(dllexport)
#else
#define AD_STREAM_EXPORT __declspec(dllimport)
#endif
#ifdef __cplusplus
extern "C" {
#endif
AD_STREAM_EXPORT void AdStreamSurfacePluginCApiRegisterWithRegistrar(
    FlutterDesktopPluginRegistrarRef registrar);
#ifdef __cplusplus
}
#endif
#endif
