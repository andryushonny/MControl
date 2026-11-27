"""Events the server sends to every client, and one line of text for each.

    command    {state: queued|started|done|failed, n, by, role, cmd, args, result, summary, error, ms}
               every action of the operator (or its assistant): moves, snaps, light, laser, camera settings, stop.
               n numbers them in the order the server runs them; begin / end mark a block
    message    {n, from, role, to, data, reply_to}   between clients (e.g. MStitch and the operator's script)
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
    if "t_wall_ms" in ev:                       # from history(): the time it happened
        t = time.strftime("%H:%M:%S", time.localtime(ev["t_wall_ms"] / 1000))
    num = f"#{ev['n']}" if "n" in ev else ""
    if k == "command":
        head = f"{num} {ev['by']}: {ev['cmd']} {_args(ev.get('args'))}".strip()
        st = ev.get("state")
        if st == "queued":
            return f"{t}  {head}  queued" + (f" ({ev['ahead']} ahead)" if ev.get("ahead") else "")
        if st == "started":
            return f"{t}  {head}  ..."
        if st == "failed":
            e = ev.get("error", {})
            return f"{t}  {head}  FAILED {e.get('code')} {e.get('msg')}"
        return f"{t}  {head}  {ev.get('summary') or 'ok'}  ({ev.get('ms', 0):.0f} ms)"
    if k == "message":
        re = f" (answer to #{ev['reply_to']})" if ev.get("reply_to") else ""
        return f"{t}  {num} message {ev['from']} -> {ev['to']}{re}: {ev.get('data')}"
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
