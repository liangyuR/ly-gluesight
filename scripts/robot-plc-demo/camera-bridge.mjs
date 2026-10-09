// Virtual trigger wire only. All part handshakes travel over the PLC's Modbus socket.
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
const { values } = parseArgs({ options: { config: { type: "string", default: fileURLToPath(new URL("demo.config.json", import.meta.url)) } } });
const config = JSON.parse((await readFile(values.config, "utf8")).replace(/^\uFEFF/, ""));
if (config.schemaVersion !== 1 || ![config.robot?.port, config.bridge?.debugPort].every(p => Number.isInteger(p) && p > 0 && p < 65536)) {
  throw new Error("Invalid simulator configuration");
}
const robotUrl = `http://127.0.0.1:${config.robot.port}`;
const debugUrl = `http://127.0.0.1:${config.bridge.debugPort}`;
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
let socket, counter = 0, validated = false;
const requests = new Map();
const acknowledgements = new Map();

async function connect() {
  validated = false;
  if (socket) socket.close();
  const tabs = await fetch(debugUrl + "/json/list", {signal:AbortSignal.timeout(2500)}).then(r => r.json());
  const targets = tabs.filter(t => t.type === "page" && new URL(t.url).origin === "http://tauri.localhost");
  if (targets.length !== 1) throw new Error("Expected exactly one GlueSight WebView");
  socket = new WebSocket(targets[0].webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error("WebView connection timed out")), 5000);
    socket.addEventListener("open", () => { clearTimeout(timeout); resolve(); }, {once:true});
    socket.addEventListener("error", error => { clearTimeout(timeout); reject(error); }, {once:true});
  });
  socket.addEventListener("message", event => {
    const message = JSON.parse(String(event.data));
    const request = requests.get(message.id);
    if (request) {
      requests.delete(message.id);
      clearTimeout(request.timeout);
      message.error ? request.reject(new Error(JSON.stringify(message.error))) : request.resolve(message.result);
    }
  });
  socket.addEventListener("close", () => {
    validated = false;
    for (const request of requests.values()) {
      clearTimeout(request.timeout);
      request.reject(new Error("WebView disconnected"));
    }
    requests.clear();
  });
  const location = await evaluate('window.__TAURI_INTERNALS__.invoke("records_list").then(r => r.root)');
  if (!String(location).split(/[\\/]/).includes("com.xyzrobotics.gluesight.robot-plc-demo")) {
    socket.close();
    throw new Error("Refusing to attach trigger bridge to a non-demo instance");
  }
  validated = true;
  console.log(new Date().toISOString(), "Virtual camera trigger bridge connected", location);
}

async function evaluate(expression) {
  const id = ++counter;
  const response = await new Promise((resolve, reject) => {
    const timeout = setTimeout(() => { requests.delete(id); reject(new Error("Camera bridge timed out")); }, 6000);
    requests.set(id, {resolve, reject, timeout});
    socket.send(JSON.stringify({id, method:"Runtime.evaluate", params:{expression, awaitPromise:true, returnByValue:true}}));
  });
  if (response.exceptionDetails) throw new Error(response.exceptionDetails.exception?.description ?? "WebView evaluation failed");
  return response.result?.value;
}

while (true) {
  try {
    if (!validated || !socket || socket.readyState !== WebSocket.OPEN) await connect();
    const ticket = await fetch(robotUrl + "/trigger", {signal:AbortSignal.timeout(2500)}).then(r => r.json());
    if (ticket) {
      let ack = acknowledgements.get(ticket.id);
      if (!ack) {
        const {sn, k, scenario} = ticket;
        if (!Number.isSafeInteger(sn) || !Number.isSafeInteger(k) || !["normal","gap","lostFrame","locateFail"].includes(scenario)) {
          throw new Error("Invalid virtual trigger ticket");
        }
        try {
          await evaluate(`window.__TAURI_INTERNALS__.invoke("sim_robot_trigger", ${JSON.stringify({sn,k,scenario})})`);
          ack = {id:ticket.id, ok:true};
        } catch (error) {
          ack = {id:ticket.id, ok:false, error:String(error)};
        }
        acknowledgements.set(ticket.id, ack);
        if (acknowledgements.size > 256) acknowledgements.delete(acknowledgements.keys().next().value);
        console.log(new Date().toISOString(), JSON.stringify(ack));
      }
      const reply = await fetch(robotUrl + "/trigger-ack", {method:"POST", headers:{"Content-Type":"application/json"}, body:JSON.stringify(ack), signal:AbortSignal.timeout(2500)});
      if (!reply.ok) throw new Error(await reply.text());
    }
    await sleep(40);
  } catch (error) {
    console.error(new Date().toISOString(), String(error));
    await sleep(1000);
  }
}
