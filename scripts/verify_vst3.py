"""Load a built SHELLOOP VST3 module the way a host does and inspect its factory.

Usage: python scripts/verify_vst3.py <path to the module binary>
  Linux:   SHELLOOP.vst3/Contents/x86_64-linux/SHELLOOP.so
  Windows: SHELLOOP.vst3
"""
import ctypes
import sys
from pathlib import Path


class PFactoryInfo(ctypes.Structure):
    _fields_ = [("vendor", ctypes.c_char * 64), ("url", ctypes.c_char * 256),
                ("email", ctypes.c_char * 128), ("flags", ctypes.c_int32)]


class PClassInfo(ctypes.Structure):
    _fields_ = [("cid", ctypes.c_ubyte * 16), ("cardinality", ctypes.c_int32),
                ("category", ctypes.c_char * 32), ("name", ctypes.c_char * 64)]


def verify(binary):
    if binary.stat().st_size < 100_000:
        raise ValueError("VST3 module is unexpectedly small")
    lib = ctypes.CDLL(str(binary))
    # Hosts call the platform init hook before asking for the factory.
    init_name = "InitDll" if sys.platform == "win32" else "ModuleEntry"
    init = getattr(lib, init_name, None)
    if init is not None:
        init.argtypes = [ctypes.c_void_p]
        init.restype = ctypes.c_bool
        if not init(None):
            raise ValueError(f"{init_name} reported failure")
    lib.GetPluginFactory.restype = ctypes.c_void_p
    factory = lib.GetPluginFactory()
    if not factory:
        raise ValueError("GetPluginFactory returned NULL (no CLAP entry found inside the module)")
    vtable = ctypes.cast(ctypes.cast(factory, ctypes.POINTER(ctypes.c_void_p))[0],
                         ctypes.POINTER(ctypes.c_void_p))
    get_info = ctypes.CFUNCTYPE(ctypes.c_int32, ctypes.c_void_p, ctypes.POINTER(PFactoryInfo))(vtable[3])
    count = ctypes.CFUNCTYPE(ctypes.c_int32, ctypes.c_void_p)(vtable[4])
    get_class = ctypes.CFUNCTYPE(ctypes.c_int32, ctypes.c_void_p, ctypes.c_int32,
                                 ctypes.POINTER(PClassInfo))(vtable[5])
    info = PFactoryInfo()
    get_info(factory, ctypes.byref(info))
    if info.vendor != b"Circuit Drift Labs":
        raise ValueError(f"unexpected vendor: {info.vendor!r}")
    classes = []
    for index in range(count(factory)):
        class_info = PClassInfo()
        get_class(factory, index, ctypes.byref(class_info))
        classes.append((class_info.category, class_info.name))
    if (b"Audio Module Class", b"SHELLOOP") not in classes:
        raise ValueError(f"SHELLOOP audio class missing from factory: {classes}")
    print(f"Verified VST3 factory: {info.vendor.decode()} / {[n.decode() for _, n in classes]}")


if __name__ == "__main__":
    verify(Path(sys.argv[1]).resolve())
