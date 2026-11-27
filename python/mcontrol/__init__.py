"""mcontrol: Python client for mc-server.

    from mcontrol import Microscope
    with Microscope() as m:              # finds the server, remembers it
        f = m.snap(x=100, y=50)          # move, settle, a fresh still frame
        img = f.rgb()                    # numpy HxWx3
"""
from .errors import McError
from .frame import Frame
from .microscope import Microscope, Viewer

__all__ = ["Microscope", "Viewer", "Frame", "McError"]
__version__ = "0.1.0"
