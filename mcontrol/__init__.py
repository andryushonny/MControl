"""mcontrol: Python client for mc-server.

    from mcontrol import Microscope
    with Microscope() as m:              # finds the server, remembers it
        m.goto(x=100, y=50)
        m.sleep(0.05)
        f = m.snap()                     # a fresh still frame, with where it was taken
        img = f.rgb()                    # numpy HxWx3
"""
from .errors import McError
from .frame import Frame
from .events import describe
from .microscope import Assistant, Microscope, Viewer

__all__ = ["Microscope", "Viewer", "Assistant", "Frame", "McError", "describe"]
__version__ = "0.6.8"
