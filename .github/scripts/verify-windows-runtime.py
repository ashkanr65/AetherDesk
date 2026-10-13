"""Load the portable native components without the build SDK on PATH."""
import ctypes
import os
from pathlib import Path
import sys
from verify_x11_runtime import verify_x11_runtime

package = Path(sys.argv[1]).resolve(strict=True)
verify_x11_runtime(package)
os.environ['PATH'] = str(package) + os.pathsep + str(Path(os.environ['SystemRoot']) / 'System32')
os.environ['GST_PLUGIN_PATH'] = str(package / 'gstreamer-1.0')
os.environ['GST_PLUGIN_SYSTEM_PATH_1_0'] = str(package / 'gstreamer-1.0')
os.environ['GST_PLUGIN_SCANNER'] = str(package / 'gst-plugin-scanner.exe')
os.environ['GST_REGISTRY'] = str(Path(os.environ['RUNNER_TEMP']) / 'aetherdesk-package-registry.bin')

with os.add_dll_directory(str(package)):
    core = ctypes.CDLL(str(package / 'ad_ffi.dll'))
    surface = ctypes.CDLL(str(package / 'ad_stream_surface_plugin.dll'))
    for symbol in ['ad_remote_start', 'ad_remote_command', 'ad_remote_status',
                   'ad_remote_acquire_frame', 'ad_remote_wait_frame', 'ad_remote_release_frame',
                   'ad_remote_close', 'ad_remote_end', 'ad_ssh_write_bytes', 'ad_ssh_read_bytes']:
        getattr(core, symbol)
    getattr(surface, 'AdStreamSurfacePluginCApiRegisterWithRegistrar')
    def load_runtime(name):
        for candidate in [name, 'lib' + name]:
            if (package / candidate).is_file():
                return ctypes.CDLL(str(package / candidate))
        raise RuntimeError('Missing packaged runtime library: ' + name)

    gst = load_runtime('gstreamer-1.0-0.dll')
    gobject = load_runtime('gobject-2.0-0.dll')
    gst.gst_init.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
    gst.gst_init.restype = None
    gst.gst_element_factory_make.argtypes = [ctypes.c_char_p, ctypes.c_char_p]
    gst.gst_element_factory_make.restype = ctypes.c_void_p
    gobject.g_object_unref.argtypes = [ctypes.c_void_p]
    gobject.g_object_unref.restype = None
    gst.gst_init(None, None)
    for name in [b'appsrc', b'appsink', b'queue', b'videoconvert', b'h264parse', b'avdec_h264']:
        element = gst.gst_element_factory_make(name, None)
        if not element:
            raise RuntimeError('Missing packaged decoder element: ' + name.decode())
        gobject.g_object_unref(element)
        print('Loaded packaged element:', name.decode())
    for relative in ['aetherdesk.exe', 'agent/linux-x86_64/ad-agent', 'agent/linux-x86_64/runtime-manifest.json']:
        if not (package / relative).is_file():
            raise RuntimeError('Missing package file: ' + relative)
    print('Portable native runtime check passed.')
