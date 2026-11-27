class McError(Exception):
    """Error from the server (or the link), with its code: E1xx net, E2xx stage, E3xx controller, E5xx camera."""

    def __init__(self, code: str, msg: str = "", detail: str = ""):
        self.code, self.msg, self.detail = code, msg, detail
        super().__init__(f"{code} {msg}" + (f": {detail}" if detail else ""))

    @classmethod
    def from_reply(cls, e: dict) -> "McError":
        return cls(e.get("code", "E000"), e.get("msg", ""), e.get("detail", ""))

    @property
    def link(self) -> bool:
        """Lost or silent link (worth a reconnect), not a refusal."""
        return self.code in ("E109", "E110")
