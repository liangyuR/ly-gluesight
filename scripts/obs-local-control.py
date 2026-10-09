"""Control an installed OBS instance through its authenticated loopback WebSocket."""
import argparse
import base64
import hashlib
import json
import os
import uuid
from pathlib import Path

import websocket


CONFIG = Path(os.environ["APPDATA"]) / "obs-studio" / "plugin_config" / "obs-websocket" / "config.json"


class OBS:
    def __init__(self, port=18955):
        config = json.loads(CONFIG.read_text(encoding="utf-8-sig"))
        self.ws = websocket.create_connection("ws://127.0.0.1:" + str(port), timeout=20,
                                              http_proxy_host=None, http_proxy_port=None)
        hello = json.loads(self.ws.recv())
        if hello["op"] != 0:
            raise RuntimeError("Unexpected OBS handshake")
        identification = {"rpcVersion": 1, "eventSubscriptions": 0}
        auth = hello["d"].get("authentication")
        if auth:
            secret = base64.b64encode(hashlib.sha256((config["server_password"] + auth["salt"]).encode()).digest()).decode()
            identification["authentication"] = base64.b64encode(hashlib.sha256((secret + auth["challenge"]).encode()).digest()).decode()
        self.ws.send(json.dumps({"op": 1, "d": identification}))
        if json.loads(self.ws.recv())["op"] != 2:
            raise RuntimeError("OBS authentication failed")

    def call(self, request, data=None):
        request_id = str(uuid.uuid4())
        self.ws.send(json.dumps({"op": 6, "d": {"requestType": request, "requestId": request_id,
                                                "requestData": data or {}}}, ensure_ascii=False))
        while True:
            result = json.loads(self.ws.recv())
            if result["op"] == 7 and result["d"]["requestId"] == request_id:
                response = result["d"]
                if not response["requestStatus"]["result"]:
                    raise RuntimeError(request + ": " + response["requestStatus"].get("comment", "failed"))
                return response.get("responseData", {})

    def close(self):
        self.ws.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("enable", "disable", "call"))
    parser.add_argument("--port", type=int, default=18955)
    parser.add_argument("--request")
    parser.add_argument("--data-file", type=Path)
    parser.add_argument("--result", type=Path)
    args = parser.parse_args()
    if args.action in ("enable", "disable"):
        config = json.loads(CONFIG.read_text(encoding="utf-8-sig"))
        if not config.get("auth_required") or len(config.get("server_password", "")) < 6:
            raise RuntimeError("OBS requires an existing valid authenticated configuration")
        config["server_enabled"] = args.action == "enable"
        CONFIG.write_text(json.dumps(config, ensure_ascii=False, indent=4), encoding="utf-8")
        print(json.dumps({"serverEnabled": config["server_enabled"], "authenticationPreserved": True}))
        return
    obs = OBS(args.port)
    try:
        data = json.loads(args.data_file.read_text(encoding="utf-8-sig")) if args.data_file else {}
        result = obs.call(args.request, data)
        if args.result:
            args.result.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        if "imageData" in result:
            result = {k: v for k, v in result.items() if k != "imageData"}
        print(json.dumps(result, ensure_ascii=False))
    finally:
        obs.close()


if __name__ == "__main__":
    main()
