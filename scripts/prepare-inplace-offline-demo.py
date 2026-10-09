"""Back up, normalize, verify, then replace the explicitly selected sample files in place."""
import argparse
import csv
import hashlib
import json
import re
import subprocess
import zipfile
from datetime import datetime, timezone, timedelta
from pathlib import Path


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def write_json(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')


def prepare(root, normalizer):
    root = root.resolve(strict=True)
    normalizer = normalizer.resolve(strict=True)
    if (root / 'dataset.json').exists():
        raise ValueError('Already packaged: verify the existing manifest instead of converting it again.')
    original = sorted(p for p in root.rglob('*') if p.is_file())
    if len(original) != 457:
        raise ValueError(f'Expected the inspected 457 original files; found {len(original)}')
    rows = []
    for group, count in [('Glue1', 80), ('Glue2', 71)]:
        files = list((root / group).iterdir())
        if len(files) != count * 3:
            raise ValueError('Unexpected group count: ' + group)
        seen = set()
        for path in files:
            match = re.fullmatch(r'Frame(\d+)_(\d+)\.jpg', path.name)
            if not match: raise ValueError('Unexpected original name: ' + str(path))
            sequence, channel = map(int, match.groups())
            if not 1 <= sequence <= count or channel not in (1, 2, 3):
                raise ValueError('Unexpected frame or channel')
            seen.add((sequence, channel))
            rows.append(dict(group=group, sequence=sequence, channel=channel,
                source=path.relative_to(root).as_posix(),
                target=f'{group}/cam{channel}_{sequence:06d}.png', sourceSha256=digest(path)))
        if len(seen) != count * 3: raise ValueError('Duplicate original frame')
    rows.sort(key=lambda row: (row['group'], row['sequence'], row['channel']))
    calibration = root / '标定' / '标定.jpg'
    rows.append(dict(group='calibration', sequence='', channel='',
        source=calibration.relative_to(root).as_posix(), target='标定/calibration-board.png',
        sourceSha256=digest(calibration)))
    stamp = datetime.now(timezone.utc).astimezone(timezone(timedelta(hours=8))).strftime('%Y%m%d-%H%M%S')
    backup = root.parent / (root.name + '_原始备份_' + stamp + '.zip')
    originals = {p.relative_to(root).as_posix(): digest(p) for p in original}
    with zipfile.ZipFile(backup, 'x', compression=zipfile.ZIP_STORED) as archive:
        for path in original: archive.write(path, path.relative_to(root).as_posix())
    with zipfile.ZipFile(backup) as archive:
        if len(archive.namelist()) != len(originals) or archive.testzip(): raise ValueError('Backup integrity error')
        for name, expected in originals.items():
            if hashlib.sha256(archive.read(name)).hexdigest() != expected:
                raise ValueError('Backup hash mismatch: ' + name)
    backup_hash = digest(backup)
    write_json(backup.with_suffix('.manifest.json'), {'archiveSha256':backup_hash,'files':originals})
    print(json.dumps({'stage':'backup-verified','archive':str(backup),'files':len(originals)}, ensure_ascii=False), flush=True)
    stage = root / ('_整理暂存_' + stamp)
    stage.mkdir()
    operations = [dict(source=str(root / row['source']), destination=str(stage / row['target'])) for row in rows]
    write_json(stage / 'operations.json', operations)
    subprocess.run([str(normalizer), str(stage / 'operations.json'), str(stage / 'pixel-check.json')], check=True)
    verification = json.loads((stage / 'pixel-check.json').read_text(encoding='utf-8'))
    if verification['converted'] != len(rows) or not all(r['softwarePixelsEqual'] for r in verification['rows']):
        raise ValueError('Incomplete software pixel verification')
    for row, checked in zip(rows, verification['rows']):
        row.update(width=checked['width'], height=checked['height'], mode='L8',
                   targetSha256=digest(stage / row['target']), softwarePixelsEqual=True)
        if row['group'] != 'calibration' and (row['width'], row['height']) != (1280,1024):
            raise ValueError('Unexpected frame dimensions')
        if (root / row['target']).exists(): raise ValueError('Target collision')
        if digest(root / row['source']) != row['sourceSha256']: raise ValueError('Source changed during conversion')
    # Install every verified output before removing any source. The verified zip is the rollback source.
    for row in rows:
        destination = root / row['target']
        source = stage / row['target']
        if not destination.resolve().is_relative_to(root) or not source.resolve().is_relative_to(stage):
            raise ValueError('Target escaped the selected dataset')
        source.replace(destination)
    for row in rows:
        source = (root / row['source']).resolve(strict=True)
        if not source.is_relative_to(root): raise ValueError('Source escaped dataset')
        if digest(root / row['target']) != row['targetSha256']: raise ValueError('Installed output mismatch')
        if digest(source) != row['sourceSha256']: raise ValueError('Source changed before replacement')
        source.unlink()
    with (root / 'frame-map.csv').open('w', encoding='utf-8-sig', newline='') as output:
        writer = csv.DictWriter(output, fieldnames=list(rows[0]))
        writer.writeheader(); writer.writerows(rows)
    groups=[]
    for group, count, title in [('Glue1',80,'直胶'),('Glue2',71,'波浪胶')]:
        write_json(root / group / 'part.json', {'startedTs':0,'syntheticTiming':True,
            'note':'100 ms/frame is a demo clock, not an original capture timestamp.',
            'frames':[{'file':f'cam{channel}_{sequence:06d}.png','ts':(sequence-1)*100}
                      for sequence in range(1,count+1) for channel in (1,2,3)]})
        same=sum(len({r['sourceSha256'] for r in rows if r['group']==group and r['sequence']==s})==1 for s in range(1,count+1))
        groups.append({'id':group,'name':title,'replayDir':group,'channels':[1,2,3],
            'framesPerChannel':count,'totalFiles':count*3,'identicalChannelTriplets':same,
            'timeline':'part.json','demoFps':10})
    for name in ['演示配置','复测结果']: (root/name).mkdir(exist_ok=True)
    manifest={'schemaVersion':1,'datasetId':'hikvision-follow-demo-20261009','name':'海康随动离线演示数据',
        'preparedAt':datetime.now(timezone.utc).astimezone(timezone(timedelta(hours=8))).isoformat(),
        'frames':453,'format':'PNG','pixelFormat':'8-bit grayscale','imageSize':[1280,1024],
        'naming':'cam{channel}_{sequence:06d}.png','groups':groups,
        'sourceBackup':'../'+backup.name,'sourceBackupSha256':backup_hash,'backupFilesVerified':457,
        'nameMap':'frame-map.csv','calibrationReference':'标定/calibration-board.png',
        'models':[p.relative_to(root).as_posix() for p in sorted((root/'模型').glob('*.bin'))],
        'normalizerSha256':digest(normalizer),'decoder':verification['decoder'],
        'softwarePixelEqualityVerified':454,'calibration':'No physical scale supplied; demo scale is 1 numerical unit per pixel.',
        'limitations':['Three channels contain identical images at corresponding frame indices.',
            'Original timestamps, measured motion, physical calibration and human OK/NG labels are not supplied.',
            'Vendor BIN models are preserved as references and are not loaded by GlueSight.',
            'GlueSight currently labels these numeric fields mm; this demo explicitly interprets them as px.']}
    write_json(root/'dataset.json',manifest)
    # Keep a concise audit without stale temporary absolute paths.
    write_json(root/'复测结果'/'conversion-check.json',{'converted':454,'softwarePixelsEqual':True,
        'decoder':verification['decoder'],'backupFilesVerified':457,'sourceBackupSha256':backup_hash,
        'modelsUnchanged':all(digest(root/name)==value for name,value in originals.items() if name.startswith('模型/'))})
    for path in sorted(stage.rglob('*'), key=lambda p:len(p.parts), reverse=True):
        if not path.resolve().is_relative_to(stage.resolve()): raise ValueError('Unexpected staging path')
        if path.is_file(): path.unlink()
        elif path.is_dir(): path.rmdir()
    stage.rmdir()
    print(json.dumps({'stage':'complete','root':str(root),'frames':453,'imagesConverted':454,'backup':str(backup)},ensure_ascii=False))


if __name__ == '__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root',type=Path,required=True)
    parser.add_argument('--normalizer',type=Path,required=True)
    args=parser.parse_args()
    prepare(args.root,args.normalizer)
