#!/usr/bin/env python3
"""Local stand-in for the EcoFlow app API + station, used by the ignored
`cloud_end_to_end` test. Needs mosquitto (broker) and mosquitto_sub/pub.

    mosquitto -p 18883 &          # anonymous local broker (see mosquitto.conf below)
    python3 tests/mock_cloud.py   # HTTP API on :18080, fake station on the broker
"""
import base64, json, subprocess, threading
from http.server import BaseHTTPRequestHandler, HTTPServer

USER_ID = "1234567890"
SN = "R621ZEB4ZE123456"
MQTT_PORT = "18883"


class Api(BaseHTTPRequestHandler):
    def _send(self, code, obj):
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        req = json.loads(self.rfile.read(int(self.headers.get("content-length", 0)) or 0) or b"{}")
        assert self.path == "/auth/login", self.path
        assert req["scene"] == "IOT_APP" and req["userType"] == "ECOFLOW", req
        if req["email"].startswith("down@"):
            return self._send(503, {"message": "busy"})
        if base64.b64decode(req["password"]).decode() != "secret":
            return self._send(200, {"code": "2026", "message": "Password error"})
        self._send(200, {"code": "0", "message": "Success",
                         "data": {"token": "tok-1", "user": {"userId": USER_ID, "name": "Test"}}})

    def do_GET(self):
        assert self.path.startswith("/iot-auth/app/certification"), self.path
        assert self.headers.get("authorization") == "Bearer tok-1", self.headers
        assert f"userId={USER_ID}" in self.path, self.path
        self._send(200, {"code": "0", "message": "Success",
                         "data": {"url": "127.0.0.1", "port": MQTT_PORT, "protocol": "mqtt",
                                  "certificateAccount": "app-user", "certificatePassword": "app-pass"}})

    def log_message(self, *a):
        pass


def station():
    """Answer latestQuotas like the cloud does, then push a partial update."""
    get_topic = f"/app/{USER_ID}/{SN}/thing/property/get"
    sub = subprocess.Popen(["mosquitto_sub", "-p", MQTT_PORT, "-t", get_topic], stdout=subprocess.PIPE, text=True)
    for line in sub.stdout:
        req = json.loads(line)
        assert req["operateType"] == "latestQuotas", req
        reply = {"id": req["id"], "version": "1.1", "operateType": "latestQuotas",
                 "data": {"online": 1, "sn": SN, "quotaMap": {
                     "pd.soc": 78, "bms_bmsStatus.soc": 78, "pd.wattsInSum": 342,
                     "pd.wattsOutSum": 40, "bms_emsStatus.chgRemainTime": 25,
                     "bms_emsStatus.dsgRemainTime": 5999, "pd.remainTime": -25}}}
        pub = lambda t, m: subprocess.run(["mosquitto_pub", "-p", MQTT_PORT, "-t", t, "-m", json.dumps(m)], check=True)
        pub(f"/app/{USER_ID}/{SN}/thing/property/get_reply", reply)
        pub(f"/app/device/property/{SN}", {"id": 1, "version": "1.0", "moduleType": 1,
                                           "params": {"pd.soc": 79, "pd.wattsOutSum": 45}})


if __name__ == "__main__":
    threading.Thread(target=station, daemon=True).start()
    HTTPServer(("127.0.0.1", 18080), Api).serve_forever()
