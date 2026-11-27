"""Integration tests against `mc-server --sim`. Run: python -m pytest tests  (build first: cargo build --release)."""
import os, subprocess, sys, time, tempfile
import pytest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, "python"))
from mcontrol import Microscope, Viewer, McError  # noqa: E402

EXE = os.path.join(ROOT, "target", "release", "mc-server" + (".exe" if os.name == "nt" else ""))
HOST = "127.0.0.1"


def start(*extra):
    log = tempfile.NamedTemporaryFile("w+", suffix=".log", delete=False)   # a pipe nobody reads would block the server
    p = subprocess.Popen([EXE, "--sim", "--no-color", "--dead-ms", "1500", *extra],
                         stdout=log, stderr=subprocess.STDOUT, cwd=tempfile.gettempdir())
    p.logfile = log.name
    for _ in range(50):
        try:
            Viewer(HOST).close()
            return p
        except McError:
            time.sleep(0.1)
    p.kill()
    raise RuntimeError("server did not start")


@pytest.fixture
def server():
    p = start("--no-guard")
    yield p
    p.kill()
    p.wait()


def test_snap_and_metadata(server):
    with Microscope(HOST) as m:
        f = m.snap(x=100, y=50, settle_ms=20)
        assert not f.moving and f.settled_ms >= 20
        assert abs(f.xyz_um[0] - 100) < 6 and abs(f.xyz_um[1] - 50) < 6
        assert f.raw().shape == f.shape and f.rgb().shape[2] == 3
        assert m.snap().seq > f.seq                       # no move: next fresh frame


def test_goto_is_idempotent(server):
    with Microscope(HOST) as m:
        a = m.stage.goto(x=200, y=-100)
        assert m.stage.goto(x=200, y=-100) == a == m.stage.position


def test_roles(server):
    with Microscope(HOST) as m:
        with pytest.raises(McError) as e:
            Microscope(HOST)
        assert e.value.code == "E103"
        with Viewer(HOST) as v:
            with pytest.raises(McError) as e:
                v.link.call("goto", x=1)
            assert e.value.code == "E102"
            assert v.latest().is_preview
            assert not v.fetch().is_preview                # one full frame per second
            with pytest.raises(McError) as e:
                v.fetch()
            assert e.value.code == "E108"
        m.release()
        Microscope(HOST).close()                           # lease is free again


def test_many_viewers(server):
    vs = [Viewer(HOST, name=f"v{i}") for i in range(8)]
    with Microscope(HOST) as m:
        t = time.time()
        n = 0
        for f in m.frames():
            n += 1
            if time.time() - t > 2:
                break
        assert n / 2 > 15                                  # raw stream keeps up while 8 viewers watch
    for v in vs:
        assert v.latest().is_preview
        v.close()


def test_silent_operator_loses_lease(server):
    m = Microscope(HOST)
    m.link.closed = True                                   # stop the heartbeat, keep the socket
    time.sleep(2.5)
    Microscope(HOST).close()                               # E105 freed the lease


def test_reconnect_repeats_only_idempotent(server):
    m = Microscope(HOST)
    m.stage.goto(x=10)
    server.kill(); server.wait()
    p = start("--no-guard")
    try:
        assert m.stage.goto(x=10, y=0, z=0)[0] == pytest.approx(10, abs=6)   # reconnected + repeated
        # link drops in the middle of a relative move: reconnect, but never repeat it by itself
        import threading
        def crash():
            time.sleep(0.3)
            p.kill(); p.wait()
        threading.Thread(target=crash).start()
        with pytest.raises(McError) as e:
            m.stage.move(dx=3000)                          # ~1 s at 3000 um/s
        assert e.value.code == "E110"
        p = start("--no-guard")
        assert m.stage.position is not None                # link is up again
        m.close()
    finally:
        p.kill()


def test_guard_makes_it_safe():
    p = start()
    with Microscope(HOST) as m:
        m.light.level = 3
    if os.name == "nt":
        subprocess.call(["taskkill", "/F", "/T", "/PID", str(p.pid)])
        return
    os.kill(p.pid, 19)                                     # SIGSTOP: server hangs, guard must act
    time.sleep(4.5)
    os.kill(p.pid, 9)
    out = open(p.logfile, errors="replace").read()
    assert "safe state set" in out
