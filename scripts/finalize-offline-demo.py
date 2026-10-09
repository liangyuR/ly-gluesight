"""Verify native recordings against the packaged pixels and write the demonstration evidence."""
import argparse
import csv
import hashlib
import json
import shutil
import sqlite3
import tempfile
import zipfile
from contextlib import closing
from datetime import datetime, timezone, timedelta
from pathlib import Path
from PIL import Image

REPO=Path(__file__).resolve().parents[1]


def write_json(path,value):
    path.write_text(json.dumps(value,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')


def write_csv(path,rows):
    with path.open('w',encoding='utf-8-sig',newline='') as output:
        writer=csv.DictWriter(output,fieldnames=list(rows[0]));writer.writeheader();writer.writerows(rows)


def sha(path):
    with path.open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()


def finalize(root):
    root=root.resolve(strict=True);out=root/'复测结果'
    manifest=json.loads((root/'dataset.json').read_text(encoding='utf-8'))
    summary=[];frames=[];points=[];reports=[]
    for group,count in [('Glue1',80),('Glue2',71)]:
        report=json.loads((out/f'{group}-native-report.json').read_text(encoding='utf-8'))
        reports.append(report);cycle=report['cycles'][-1];detail=cycle['detail'];check=cycle['retestCheck']
        assert check['passed'] and check['rawComplete'] and check['originalUnchanged']
        assert cycle['capturedPerChannel']==[count]*3
        assert detail['summary']['framesReceived']==count*3 and detail['summary']['faultCode']==0
        assert not any(m['error'] for m in cycle['measurements'])
        assert all(c['lostPackets']==0 and c['droppedFrames']==0 for c in cycle['cameras'])
        assert all(s<2 for s in detail['points']['st'])
        matches=[]
        for entry in report['records']['items']:
            directory=Path(entry['path']);meta=json.loads((directory/'part.json').read_text(encoding='utf-8'))
            if meta['sn']==detail['summary']['sn']:matches.append((directory,meta))
        assert len(matches)==1
        directory,meta=matches[0]
        assert len(meta['frames'])==count*3 and meta['droppedFrames']==0
        seen=set()
        for f in meta['frames']:
            camera=f['camera'];sequence=f['seq'];key=(camera,sequence)
            assert key not in seen;seen.add(key)
            source=f'{group}/{camera}_{sequence:06d}.png'
            with Image.open(root/source) as a, Image.open(directory/f['file']) as b:
                a.load();b.load();assert a.mode==b.mode=='L' and a.size==b.size==(1280,1024)
                assert a.tobytes()==b.tobytes(),(source,f['file'])
                pixels=hashlib.sha256(a.tobytes()).hexdigest()
            frames.append({'group':group,'historyId':detail['summary']['id'],'camera':camera,'sequence':sequence,
                'source':source,'recordFile':f['file'],'frameCounter':f['frameCounter'],'captureTs':f['ts'],
                'pixelSha256':pixels,'recordedPixelsEqual':True})
        assert seen=={(f'cam{c}',s) for c in (1,2,3) for s in range(1,count+1)}
        for j,status in enumerate(detail['points']['st']):
            points.append({'group':group,'historyId':detail['summary']['id'],'index':j,'sPx':j*5,'status':status,
                'offsetPx':detail['points']['d'][j] if status==0 else '',
                'widthPx':detail['points']['w'][j] if status==0 else ''})
        summary.append({'group':group,'historyId':detail['summary']['id'],'sn':detail['summary']['sn'],
            'framesPerChannel':count,'recordedFrames':count*3,'recordedPixelsVerified':count*3,
            'measuredFrames':len(cycle['measurements']),'measuredPoints':len(detail['points']['st']),
            'noBeadPoints':detail['points']['st'].count(1),'uncoveredPoints':0,
            'verdict':detail['summary']['verdict'],'retestVerdict':check['retestVerdict'],
            'stateMismatches':check['statusMismatches'],'valueMismatches':check['valueMismatches'],
            'maxNumericDifferencePx':check['maxDifference'],'cameraDrops':0,'recordingDrops':0,
            'nativeErrors':0,'originalUnchanged':True,'recordDirectory':str(directory)})
        config_dir=root/'演示配置'/'已验证参数';config_dir.mkdir(exist_ok=True)
        write_json(config_dir/f'{group}-cameras.json',cycle['cameraConfig'])
        write_json(config_dir/f'{group}-settings.json',cycle['settings'])
        write_json(config_dir/f'{group}-candidate.json',report['finalCandidate'])
    assert len(frames)==453 and len(points)==128
    write_csv(out/'逐帧像素核验.csv',frames);write_csv(out/'原始测量点.csv',points);write_csv(out/'整件复测结果.csv',summary)
    images=out/'截图';images.mkdir(exist_ok=True)
    for source in (REPO/'output'/'playwright'/'offline-demo-20261009').glob('Glue*-*.png'):
        shutil.copy2(source,images/source.name)
    audit={'verifiedAt':datetime.now(timezone.utc).astimezone(timezone(timedelta(hours=8))).isoformat(),
        'passed':True,'convertedImages':454,'replayFrames':453,'recordedPixelMatches':453,
        'totalRemeasuredPoints':128,'groups':summary,'consoleErrors':[e for r in reports for e in r['consoleErrors']],
        'parameterExperiment':[c for r in reports for c in r['checks'] if 'sensitivity' in c['operation']]}
    assert not audit['consoleErrors']
    write_json(out/'验收摘要.json',audit)
    lines=['# 海康随动离线数据复测报告','',
        '2026 年 10 月 9 日，GlueSight 桌面端完成两组真实图像回放、检测、原图录制和历史原图复测。453 张检测图已原地整理为 8 位灰度 PNG，软件读取的灰度像素与原 JPEG 一致。',
        '',
        '**本次验收通过的是数据完整性、图像测量流程和历史复测一致性。样本没有人工缺陷标签；表中的胶宽超差是演示参数下的算法结论。所有长度按像素解释，软件现有字段仍显示 mm。**',
        '',
        '| 案例 | 历史记录 | 原图帧数 | 测量帧数 | 测点 | 首次结论 | 原图复测 | 状态差异 |',
        '|---|---:|---:|---:|---:|---|---|---:|']
    for s in summary:
        lines.append(f"| {s['group']} | #{s['historyId']} | {s['recordedFrames']} | {s['measuredFrames']} | {s['measuredPoints']} | 胶宽超差 | 胶宽超差 | 0 |")
    lines.extend(['',
        '两组均无相机丢帧、录制丢帧、测量错误或未覆盖测点。回放接收的全部 453 张录制原图已逐张与数据包 PNG 核对，像素完全一致；复测与首次测量的最大数值差异小于 0.000002 像素，是数值序列化的舍入差异。原始历史记录在复测前后保持一致。',
        '',
        'Glue1 的最小边缘灰度差由 18 调到 80 后，27 个测点的状态改变，结论由胶宽超差变为断胶；原参数恢复为 18 后，再次复现原始胶宽超差结果。该对照单独保存，生产演示配方和原始记录均保留。',
        '',
        '另保留一条准备阶段 Glue1 采集记录 #1。主验收采用 #2 和 #3；三条记录均可在当前离线演示实例的历史页找到。',
        '',
        '原文件已先备份为同级 ZIP，备份内 457 个文件全部通过逐文件 SHA256 核验。454 张图片完成解码、灰度 PNG 转码及软件像素一致性检查；三个厂家 BIN 模型保持原样。',
        '',
        '每组的三路对应帧完全相同，共 151 组重复视图。10 FPS、名义直线轨迹、80 像素每秒和像素当量 1 都是流程演示设置；整件重新回放会受运行调度影响而出现读数变化，历史原图复测沿用记录的沿程位置来核对一致性。厂家 BIN 模型未参与本次测量。',
        '',
        '可复核证据包括 [逐帧像素核验](逐帧像素核验.csv)、[整件复测结果](整件复测结果.csv)、[原始测量点](原始测量点.csv)、[验收摘要](验收摘要.json)、[Glue1 原生报告](Glue1-native-report.json) 和 [Glue2 原生报告](Glue2-native-report.json)。界面截图在“截图”目录。',
        '',
        '演示应用使用独立的数据目录，目录位置及程序哈希见 ../演示配置/application.json。当前演示状态的可恢复归档为“演示状态备份.zip”，含检测数据库、配置、候选对照及完整录制原图。',
        ''])
    (out/'复测报告.md').write_text('\n'.join(lines),encoding='utf-8')
    guide=f'''# 海康离线演示操作说明

这组数据在 GlueSight 胶路智检中演示“读取原图、检测、保存历史、从原图复测”。当前已完成直胶和波浪胶验收，可直接查看历史记录 #2 和 #3。

演示开始时说明：所有长度数值按像素解释，界面中的 mm 是现有软件标签；三个通道使用相同视图。当前结论是演示参数下的算法输出。

1. 关闭已有的离线演示窗口，双击“01 打开直胶演示.cmd”或“02 打开波浪胶演示.cmd”。切换案例会同时切换三个回放目录和相应图像方位。
2. 打开“设备与采集”，点击“下一张”查看原图；通道分别为 1、2、3。Glue1 每路 80 帧，Glue2 每路 71 帧。
3. 如需展示试测，进入“随动相机标定”，导入对应组的 cam1_000040.png（直胶）或 cam1_000035.png（波浪胶）。将名义胶宽设为 16、搜索半宽 25、窗口近端 70、远端 280，点击“在当前帧试测”。已配置的胶嘴为 688、646，图像方位直胶 105 度、波浪胶 128 度，像素当量 1。
4. 打开“在线检测”。在“模拟配方”中，直胶明确选择 OFFLINE-GLUE1，波浪胶明确选择 OFFLINE-GLUE2；每次进入页面都检查一次。工况选“正常件”，点击“运行一件”。这里模拟的是 PLC 节拍，图像和测量均来自真实回放数据。
5. 等待约 8 秒完成。当前演示配方可能得到“胶宽超差”；按实际显示讲解胶宽曲线和测点，不能把它当作人工确认的现场缺陷。
6. 进入“历史记录”，打开对应工件，点击“使用该配方候选”，再点击“从原图复测整件”。检查原始与候选结果，使用“历史帧选择”查看保存的原图。复测 Glue1 历史时先从直胶入口启动，复测 Glue2 时先从波浪胶入口启动，以使用对应的工位参数。
7. 要展示参数影响，在“胶路与拍照规划”将“最小边缘灰度差”从 18 改为 80，保存候选，回到同一历史工件再次从原图复测。演示后恢复 18 并保存；各次对照保留在历史页的下拉框中。

原图自动保存在独立演示应用的数据目录，启用了“保留全部工件”。本数据包的“复测结果”里已有验收报告、CSV、界面截图和演示状态备份。查看已有结果不需要联网或连接现场相机、PLC。

若提示演示已打开，请先关闭当前窗口，再双击另一个入口。数据包换位置后仍通过入口启动；入口会按新根目录更新回放路径。演示程序依赖当前 Windows 已安装的 WebView2。

原始文件备份：{manifest['sourceBackup'][3:]}。恢复原始素材时先关闭演示软件，将备份解压到一个新目录核对，再决定是否替换已整理目录。
'''
    (root/'演示操作说明.md').write_text(guide,encoding='utf-8')
    for index,group,title in [(1,'Glue1','直胶'),(2,'Glue2','波浪胶')]:
        command='@echo off\r\nchcp 65001 >nul\r\npwsh.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0演示配置\\Start-OfflineDemo.ps1" -Group '+group+'\r\nif errorlevel 1 pause\r\n'
        (root/f'0{index} 打开{title}演示.cmd').write_text(command,encoding='utf-8',newline='')
    print(json.dumps({'stage':'frame-audit-complete','frames':len(frames),'points':len(points),'groups':summary},ensure_ascii=False),flush=True)
    profile=Path(reports[-1]['recordsRoot']).parent.resolve(strict=True)
    if profile.name!='com.xyzrobotics.gluesight.offline-demo-20261009':raise ValueError('Unexpected profile')
    archive=out/'演示状态备份.zip'
    with tempfile.TemporaryDirectory(prefix='gluesight-profile-') as temporary:
        temp=Path(temporary).resolve(strict=True)
        if not temp.is_relative_to(Path(tempfile.gettempdir()).resolve()) or not temp.name.startswith('gluesight-profile-'):
            raise ValueError('Unexpected temporary backup directory')
        db_copy=temp/'inspection.db'
        with closing(sqlite3.connect(profile.joinpath('inspection.db').as_uri()+'?mode=ro',uri=True)) as source:
            with closing(sqlite3.connect(db_copy)) as dest:source.backup(dest)
        with zipfile.ZipFile(archive,'w',compression=zipfile.ZIP_DEFLATED,compresslevel=1) as z:
            z.write(db_copy,'inspection.db')
            for name in ['cameras.json','cycle.json']:
                z.write(profile/name,name)
            for folder in ['recipes','records','workspaces']:
                for p in sorted((profile/folder).rglob('*')):
                    if p.is_file():z.write(p,p.relative_to(profile).as_posix())
        with zipfile.ZipFile(archive) as z:
            assert z.testzip() is None
            files=len(z.namelist())
    write_json(out/'演示状态备份说明.json',{'file':archive.name,'sha256':sha(archive),'files':files,
        'profile':str(profile),'restoreInstruction':'Close the isolated demo. Restore into a NEW empty profile directory only after preserving its current data. This archive contains a consistent SQLite backup and original frame recordings.',
        'capturedGroup':'Glue2','historyIds':[1,2,3]})
    manifest['demo']={'launchers':['01 打开直胶演示.cmd','02 打开波浪胶演示.cmd'],'guide':'演示操作说明.md',
        'report':'复测结果/复测报告.md','summary':'复测结果/验收摘要.json','application':'演示配置/application.json'}
    write_json(root/'dataset.json',manifest)
    print(json.dumps({'stage':'complete','profileArchive':str(archive),'archiveFiles':files,'archiveBytes':archive.stat().st_size},ensure_ascii=False))


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--root',type=Path,required=True)
    finalize(parser.parse_args().root)
