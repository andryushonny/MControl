"""Events the server sends to every client, and one line of text for each.

    command    {state: started|done|failed, by, cmd, args, result, summary, error, ms}
               every action of the operator: moves, snaps, light, laser, camera settings, stop
    client     {action: connected|disconnected|..., role, name, addr, after_s}
    operator   {name, addr}            who holds control now (name None: nobody)
    safe       {why}                   stage stopped and laser off because the operator was lost
    error      {by, error: {code, msg}}
    telemetry  {stage, camera, operator, clients}   once a second
    closed     {}                      our own link dropped (the client reconnects)
"""
import time


def _args(a):
    return " ".join(f"{k}={v}" for k, v in (a or {}).items())


def describe(ev: dict) -> str:
    """One readable line for an event (time on this PC)."""
    k = ev.get("event")
    t = time.strftime("%H:%M:%S")
    if k == "command":
        head = f"{ev['by']}: {ev['cmd']} {_args(ev.get('args'))}".rstrip()
        st = ev.get("state")
        if st == "started":
            return f"{t}  {head}  ..."
        if st == "failed":
            e = ev.get("error", {})
            return f"{t}  {head}  FAILED {e.get('code')} {e.get('msg')}"
        return f"{t}  {head}  {ev.get('summary') or 'ok'}  ({ev.get('ms', 0):.0f} ms)"
    if k == "client":
        return f"{t}  {ev['role']} {ev['name']} {ev['addr']} {ev['action']}"
    if k == "operator":
        return f"{t}  control: {ev.get('name') or 'free'}"
    if k == "safe":
        return f"{t}  SAFE STATE: {ev.get('why')} (stage stopped, laser off)"
    if k == "error":
        e = ev.get("error", {})
        return f"{t}  ERROR {e.get('code')} {e.get('msg')}"
    if k == "telemetry":
        st = ev.get("stage", {})
        pos = ", ".join(f"{x:.1f}" for x in st.get("pos_um", []))
        return (f"{t}  at ({pos}) um{'  moving' if st.get('moving') else ''}  light {st.get('light')}"
                f"  laser {st.get('laser_mw')} mW  camera {ev.get('camera', {}).get('fps')} fps")
    if k == "closed":
        return f"{t}  link lost, reconnecting"
    return ""
