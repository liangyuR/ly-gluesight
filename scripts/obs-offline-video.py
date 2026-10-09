"""Create an isolated OBS recording scene and restore the original selection."""
import argparse
import importlib.util
import json
from pathlib import Path

spec = importlib.util.spec_from_file_location("obs_control", Path(__file__).with_name("obs-local-control.py"))
control = importlib.util.module_from_spec(spec)
spec.loader.exec_module(control)
ROOT = Path(__file__).resolve().parents[1] / "output/offline/hikvision-three-camera/video"
STATE = ROOT / "obs-state.json"
SCENE = "离线测试窗口"
CAPTURE = "TuJiao 原生窗口"
TITLE = "离线测试标题"
NOTE = "实验条件说明"


def save(name, result):
    (ROOT / name).write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def prepare(obs):
    if STATE.exists():
        raise RuntimeError("OBS recording state already exists; do not overwrite the original selection")
    profile = obs.call("GetProfileList")["currentProfileName"]
    collection = obs.call("GetSceneCollectionList")["currentSceneCollectionName"]
    assert not obs.call("GetRecordStatus")["outputActive"]
    assert not obs.call("GetStreamStatus")["outputActive"]
    save("obs-state.json", {"originalProfile": profile, "originalCollection": collection,
                            "temporaryProfile": "TuJiao Offline 20261009",
                            "temporaryCollection": "TuJiao Offline 20261009"})
    obs.call("CreateProfile", {"profileName": "TuJiao Offline 20261009"})
    params = {"Output": {"Mode": "Simple", "FilenameFormatting": "TuJiao-Offline-%CCYY%MM%DD-%hh%mm%ss"},
              "SimpleOutput": {"RecFormat2": "hybrid_mp4", "RecQuality": "Stream", "RecEncoder": "x264",
                               "StreamEncoder": "x264", "VBitrate": "7000", "ABitrate": "160",
                               "RecTracks": "1"}}
    for category, values in params.items():
        for name, value in values.items():
            obs.call("SetProfileParameter", {"parameterCategory": category, "parameterName": name,
                                              "parameterValue": value})
    obs.call("SetRecordDirectory", {"recordDirectory": str(ROOT)})
    obs.call("SetVideoSettings", {"baseWidth": 1920, "baseHeight": 1080, "outputWidth": 1920,
                                   "outputHeight": 1080, "fpsNumerator": 30, "fpsDenominator": 1})
    obs.call("CreateSceneCollection", {"sceneCollectionName": "TuJiao Offline 20261009"})
    obs.call("CreateScene", {"sceneName": SCENE})
    obs.call("SetCurrentProgramScene", {"sceneName": SCENE})
    for name in obs.call("GetSpecialInputs").values():
        if isinstance(name, str) and name:
            obs.call("SetInputMute", {"inputName": name, "inputMuted": True})
    obs.call("CreateInput", {"sceneName": SCENE, "inputName": "深色背景", "inputKind": "color_source_v3",
                             "inputSettings": {"color": 4279705370, "width": 1920, "height": 1080}})
    capture = obs.call("CreateInput", {"sceneName": SCENE, "inputName": CAPTURE, "inputKind": "window_capture",
                                       "inputSettings": {"client_area": True, "cursor": False, "method": 2}})
    state = json.loads(STATE.read_text(encoding="utf-8"))
    state["captureItemId"] = capture["sceneItemId"]
    save("obs-state.json", state)
    for name, text, y, size, color in [
        (TITLE, "涂胶检测 · 三路离线回放测试", 15, 32, 16777215),
        (NOTE, "随动检测 | mm/px=1 为实验坐标；界面数值按 px 解读 | 100 ms 合成时序 | 三路源图相同", 1029, 26, 14671839)]:
        created = obs.call("CreateInput", {"sceneName": SCENE, "inputName": name, "inputKind": "text_gdiplus_v3",
            "inputSettings": {"text": text, "font": {"face": "Microsoft YaHei", "size": size}, "color": color}})
        obs.call("SetSceneItemTransform", {"sceneName": SCENE, "sceneItemId": created["sceneItemId"],
                                            "sceneItemTransform": {"positionX": 28, "positionY": y}})
    return {"profile": obs.call("GetProfileList"), "scene": obs.call("GetCurrentProgramScene"),
            "video": obs.call("GetVideoSettings"), "audio": obs.call("GetSpecialInputs")}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["prepare", "capture", "caption", "start", "status", "stop", "restore", "screenshot"])
    parser.add_argument("--text", default="")
    args = parser.parse_args()
    ROOT.mkdir(parents=True, exist_ok=True)
    obs = control.OBS()
    try:
        if args.action == "prepare":
            result = prepare(obs)
        elif args.action == "capture":
            items = obs.call("GetInputPropertiesListPropertyItems", {"inputName": CAPTURE, "propertyName": "window"})
            save("obs-window-options.json", items)
            targets = [item for item in items["propertyItems"] if "tujiao-offline-recording.exe" in str(item.get("itemValue", "")).lower()]
            if len(targets) != 1:
                raise RuntimeError("Expected one visible native recording window; found " + str(len(targets)))
            obs.call("SetInputSettings", {"inputName": CAPTURE, "inputSettings": {
                "window": targets[0]["itemValue"], "client_area": True, "cursor": False, "method": 2}, "overlay": True})
            state = json.loads(STATE.read_text(encoding="utf-8"))
            obs.call("SetSceneItemTransform", {"sceneName": SCENE, "sceneItemId": state["captureItemId"],
                "sceneItemTransform": {"positionX": 0, "positionY": 66, "boundsType": "OBS_BOUNDS_SCALE_INNER",
                                       "boundsWidth": 1920, "boundsHeight": 954, "boundsAlignment": 0}})
            result = {"window": targets[0], "transform": obs.call("GetSceneItemTransform", {
                "sceneName": SCENE, "sceneItemId": state["captureItemId"]})}
        elif args.action == "caption":
            result = obs.call("SetInputSettings", {"inputName": TITLE, "inputSettings": {"text": args.text}, "overlay": True})
        elif args.action == "start":
            result = obs.call("StartRecord")
        elif args.action == "status":
            result = obs.call("GetRecordStatus")
        elif args.action == "stop":
            result = obs.call("StopRecord")
            save("obs-recording.json", result)
        elif args.action == "screenshot":
            result = obs.call("SaveSourceScreenshot", {"sourceName": SCENE, "imageFormat": "png",
                "imageFilePath": str(ROOT / "obs-preview.png"), "imageWidth": 1920, "imageHeight": 1080})
        else:
            assert not obs.call("GetRecordStatus")["outputActive"]
            state = json.loads(STATE.read_text(encoding="utf-8"))
            obs.call("SetCurrentProfile", {"profileName": state["originalProfile"]})
            obs.call("SetCurrentSceneCollection", {"sceneCollectionName": state["originalCollection"]})
            result = {"profile": obs.call("GetProfileList"), "collection": obs.call("GetSceneCollectionList"),
                      "recording": obs.call("GetRecordStatus"), "stream": obs.call("GetStreamStatus")}
            save("obs-restored.json", result)
        print(json.dumps(result, ensure_ascii=False))
    finally:
        obs.close()


if __name__ == "__main__":
    main()
