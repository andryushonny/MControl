"""Integration tests against `mc-server --sim`. Run: uv run pytest  (build first: cargo build --release)."""
import os, subprocess, time, tempfile
import pytest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
os.environ["MCONTROL_CONFIG"] = os.path.join(tempfile.mkdtemp(), "mcontrol.json")   # leave the user's memory alone
from mcontrol import Microscope, Viewer, McError

EXE = os.path.join(ROOT, "target", "release", "mc-server" + (".exe" if os.name == "nt" else ""))
HOST = "127.0.0.1"


def start(*extra):
    log = tempfile.NamedTemporaryFile("w+", suffix=".log", delete=False)   # a pipe nobody reads would block the server
    p = subprocess.Popen([EXE, "--sim", "--no-color", "--dead-ms", "1500", *extra],
                         stdout=log, stderr=subprocess.STDOUT, cwd=tempfile.gettempdir())
    p.logfile = log.name
    for _ in range(100):                                   # the address table is printed once it listens
        if "Ctrl+C" in open(log.name, errors="replace").read():
            return p
        if p.poll() is not None:
            return p                                       # refused to start: the test reads the log
        time.sleep(0.05)
    p.kill()
    raise RuntimeError("server did not start")


def stop(p):
    p.kill()
    p.wait()


@pytest.fixture
def server():
    p = start("--no-guard")
    yield p
    stop(p)


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
    vs = [Viewer(HOST, name=f"v{i}") for i in range(5)]   # the default limit
    with pytest.raises(McError) as e:
        Viewer(HOST, name="v6")
    assert e.value.code == "E114"
    with Microscope(HOST) as m:
        n = sum(1 for _ in m.live(seconds=2))
        assert n / 2 > 15                                  # raw stream keeps up while 5 viewers watch
        assert not m.stream.on                             # switched off after the loop
    for v in vs:
        t, n = time.time(), sum(1 for _ in v.live(seconds=1))
        assert v.latest().is_preview and n >= 15         # viewers get ~25 fps previews
        v.close()


def test_viewers_see_what_the_operator_does(server):
    from mcontrol import describe
    with Viewer(HOST) as v, Microscope(HOST, name="op") as m:
        seen = []
        v.on_event(seen.append)
        m.light.level = 2
        m.snap(x=50)
        with pytest.raises(McError):
            m.laser.mw = 5
        time.sleep(0.3)
        cmds = [(e["cmd"], e["state"]) for e in seen if e["event"] == "command"]
        assert ("light", "started") in cmds and ("light", "done") in cmds
        assert ("snap", "done") in cmds and ("laser", "failed") in cmds
        snap = next(e for e in seen if e["event"] == "command" and e["cmd"] == "snap" and e["state"] == "done")
        assert snap["by"] == "op" and snap["result"]["meta"]["xyz_um"][0] > 40
        assert "op: snap x=50.0" in describe(snap)


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
    assert "Safe state set" in out


# ---------------------------------------------------------------- network and access

def test_token():
    p = start("--no-guard", "--token", "s3cret")
    try:
        with pytest.raises(McError) as e:
            Viewer(HOST, token="wrong")
        assert e.value.code == "E112"
        with Microscope(HOST, token="s3cret") as m:
            assert m.snap().raw().size
    finally:
        stop(p)


def test_operators_and_allow():
    p = start("--no-guard", "--operators", "10.0.0.0/8")    # this PC (127.0.0.1) may only watch
    try:
        with pytest.raises(McError) as e:
            Microscope(HOST)
        assert e.value.code == "E113"
        Viewer(HOST).close()
    finally:
        stop(p)
    p = start("--no-guard", "--allow", "10.0.0.0/8")
    try:
        with pytest.raises(McError) as e:
            Viewer(HOST)
        assert e.value.code == "E111"
    finally:
        stop(p)


def test_open_without_token_refuses_to_start():
    p = start("--no-guard", "--allow", "any")
    assert p.wait(5) == 2 and "E121" in open(p.logfile).read()


def test_busy_port_moves_and_discovery_finds_it():
    import socket
    busy = socket.socket()
    if os.name != "nt":                                    # an earlier server's port may sit in TIME_WAIT
        busy.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    busy.bind(("0.0.0.0", 5160))
    busy.listen()
    p = start("--no-guard")
    try:
        assert "Port 5160 busy" in open(p.logfile).read()
        with Microscope() as m:                            # found by discovery, on the moved port
            assert m.snap().seq > 0
    finally:
        stop(p)
        busy.close()


def test_readme_lists_every_error():
    table = subprocess.run([EXE, "errors"], capture_output=True, text=True).stdout
    assert table.strip() in open(os.path.join(ROOT, "README.md"), encoding="utf-8").read().replace("\r", "")


# ---------------------------------------------------------------- frame packing

def test_codecs(server):
    import numpy as np
    with Microscope(HOST, codec="raw") as m, Viewer(HOST) as v:
        f = m.snap()
        assert f.codec == "raw" and f.exact
        same = v.fetch(f.seq)                              # full frames to viewers go lossless
        assert same.codec == "lossless" and len(same.data) < len(f.data) / 1.3
        assert np.array_equal(same.raw(), f.raw())        # bit for bit
    with Microscope(HOST, codec="jpeg", quality=85) as m:
        f = m.snap()
        assert f.codec == "jpeg 85" and not f.exact and len(f.data) < f.meta["raw_bytes"] / 4
        assert f.raw().shape == f.shape
    with Microscope(HOST) as m:                            # auto on a fast link: exact
        assert m.snap().codec == "lossless"
        assert all(fr.codec == "lossless" for fr in m.live(count=10))


# ---------------------------------------------------------------- PC 1 keeps its devices

@pytest.mark.skipif(os.name == "nt", reason="finds the worker with pgrep")
def test_camera_worker_comes_back_with_its_settings(server):
    with Microscope(HOST) as m:
        m.camera.exposure_ms = 33
        worker = subprocess.run(["pgrep", "-P", str(server.pid), "-f", "camera-worker"], capture_output=True, text=True).stdout.split()
        os.kill(int(worker[0]), 9)                         # the camera driver dies
        time.sleep(3)
        f = m.snap()                                       # frames again, from a new worker
        assert abs(f.exposure_ms - 33) < 1                 # with the operator's exposure
        assert "starting it again" in open(server.logfile).read()


# ---------------------------------------------------------------- queue, chains, history, assistant, messages

def test_commands_run_in_order(server):
    import threading
    with Microscope(HOST) as m:
        done = []
        def worker(i):                                     # many threads at once: the server still runs them one by one
            r = m.link.call("goto", x=float(10 * i))
            done.append(r["n"])
        ts = [threading.Thread(target=worker, args=(i,)) for i in range(8)]
        [t.start() for t in ts]
        [t.join() for t in ts]
        h = [e for e in m.history(limit=50) if e["cmd"] == "goto"]
        assert [e["n"] for e in h] == sorted(e["n"] for e in h)   # history is in the order they ran
        assert sorted(done) == [e["n"] for e in h][-8:]


def test_block_and_stop(server):
    import threading
    from mcontrol import Assistant
    with Microscope(HOST) as m, Assistant(HOST, name="other") as a:
        order = []
        def other():                                      # tries to get in between, from another client
            a.goto(x=777)
            order.append("other")
        with m.block():
            m.goto(x=0, y=0)
            t = threading.Thread(target=other); t.start()
            time.sleep(0.3)
            m.sleep(0.1)
            f = m.snap(x=200)                              # results come at once, as usual
            order.append("block")
            assert f.xyz_um[0] == pytest.approx(200, abs=6)
        t.join()
        assert order == ["block", "other"]                # the other command waited until the block ended
        assert m.stage.position[0] == pytest.approx(777, abs=6)
        # stop ends a block and drops what waits behind it
        out = {}
        t = threading.Thread(target=lambda: out.setdefault("sleep", _catch(lambda: m.sleep(3))))
        t2 = threading.Thread(target=lambda: out.setdefault("goto", _catch(lambda: m.goto(x=900))))
        t.start(); time.sleep(0.3); t2.start(); time.sleep(0.3)
        m.stop()
        t.join(); t2.join()
        assert out["sleep"] == "E116" and out["goto"] == "E116"
        assert m.stage.position[0] < 800                   # the move never happened


def _catch(fn):
    try:
        fn()
        return "ok"
    except McError as e:
        return e.code


def test_assistant_and_messages(server):
    from mcontrol import Assistant
    with Microscope(HOST, name="script") as m:
        @m.on_message
        def answer(msg):                                   # the operator's script answers MStitch
            if msg["data"].get("want") == "reshoot":
                f = m.snap(x=msg["data"]["x"])
                m.reply(msg, {"seq": f.seq})
        with Assistant(HOST, name="MStitch") as a:         # same PC: no key needed
            assert a.snap(x=50).xyz_um[0] == pytest.approx(50, abs=6)   # it may act itself, through the same queue
            got = a.request("operator", {"want": "reshoot", "x": 120}, timeout=10)
            assert a.fetch(got["seq"]).xyz_um[0] == pytest.approx(120, abs=6)
        with Viewer(HOST) as v:
            assert [e for e in v.history() if e["event"] == "message"]  # messages are in the history too
    with pytest.raises(McError) as e:
        Assistant(HOST, name="late")                       # nobody in control: nothing to join
    assert e.value.code == "E115"
