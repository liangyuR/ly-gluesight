"""Package the visible isolated desktop build and reproducible presets beside the normalized images."""
import argparse
import hashlib
import json
import shutil
from pathlib import Path

REPO=Path(__file__).resolve().parents[1]
IDENTIFIER='com.xyzrobotics.gluesight.offline-demo-20261009'


def write(path,value):
    path.parent.mkdir(parents=True,exist_ok=True)
    path.write_text(json.dumps(value,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')


def package(root,exe):
    root=root.resolve(strict=True)
    if not json.loads((root/'dataset.json').read_text(encoding='utf-8'))['softwarePixelEqualityVerified']==454:
        raise ValueError('Dataset conversion has not passed')
    dest=root/'演示配置'
    previous=REPO/'output'/'offline'/'hikvision-three-camera'/'test-config'
    for group,title in [('Glue1','直胶'),('Glue2','波浪胶')]:
        doc=json.loads((previous/f'{group}-step5.recipe.json').read_text(encoding='utf-8'))
        doc['name']=f'{group} {title}离线演示（数值按 px / 合成时序）'
        doc['follow']['cameras']=['cam1','cam2','cam3']
        write(dest/'recipes'/f'{doc["id"]}.json',doc)
        cameras=json.loads((previous/f'{group}-step5.cameras.json').read_text(encoding='utf-8'))
        for index,camera in enumerate(cameras,1):
            camera['id']=f'cam{index}'
            camera['name']=f'离线通道 {index}（重复视图）'
            camera['replayDir']=group
        write(dest/group/'cameras.json',{'nextId':4,'cameras':cameras})
        write(dest/group/'calibration.json',cameras[0]['follow'])
    settings=json.loads((previous/'Glue1-step5.settings.json').read_text(encoding='utf-8'))
    settings.update(record='all',recordKeep=100,recordMaxGb=20,followVision=True,vision=False,lyflowCore=None)
    write(dest/'cycle.json',settings)
    runner=dest/'runner'/'GlueSight-Offline.exe'
    runner.parent.mkdir(exist_ok=True)
    shutil.copy2(exe,runner)
    with runner.open('rb') as stream: digest=hashlib.file_digest(stream,'sha256').hexdigest()
    write(dest/'application.json',{'identifier':IDENTIFIER,'sha256':digest,'version':'0.1.0',
        'configRoot':'%APPDATA%/'+IDENTIFIER,'measurement':'Native follow caliper reading real replay pixels',
        'physicalCalibration':False,'units':'px; existing desktop numeric labels say mm; demo scale is 1',
        'motion':'Synthetic nominal straight path and 80 px/s; not measured production motion',
        'recipeProvenance':'Independent demo presets based on inspected prior step-5 parameter snapshots; not label-validated production releases'})
    shutil.copy2(REPO/'scripts'/'Start-OfflineDemo.ps1',dest/'Start-OfflineDemo.ps1')
    print(json.dumps({'root':str(root),'executable':str(runner),'sha256':digest,'identifier':IDENTIFIER},ensure_ascii=False))


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--root',type=Path,required=True);p.add_argument('--exe',type=Path,required=True)
    args=p.parse_args();package(args.root,args.exe)
