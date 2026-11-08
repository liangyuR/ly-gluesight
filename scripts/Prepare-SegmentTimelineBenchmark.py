import argparse
import json
from pathlib import Path
import re
import shutil
import subprocess


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--workspace', type=Path, required=True)
    parser.add_argument('--second-round', required=True)
    parser.add_argument('--before-ref', required=True)
    args = parser.parse_args()
    repo = Path(__file__).resolve().parent.parent
    output = repo / 'output/perf-thumbnails/harness'
    output.mkdir(parents=True, exist_ok=True)
    workspace = json.loads(args.workspace.read_text(encoding='utf-8'))
    (output / 'workspace.json').write_text(json.dumps(workspace, ensure_ascii=False), encoding='utf-8')
    (output / 'benchmark-config.json').write_text(json.dumps({'secondRound': args.second_round}), encoding='utf-8')
    for template in (repo / 'scripts/segment-timeline-benchmark').iterdir():
        shutil.copyfile(template, output / template.name)
    for filename in ['SegmentWorkbench.tsx', 'thumbs.tsx']:
        source = subprocess.check_output(['git', 'show', f'{args.before_ref}:src/features/workspace/{filename}'], cwd=repo).decode('utf-8')

        def rewrite(match):
            target = match.group(1)
            if not target.startswith('.'):
                return match.group(0)
            if filename == 'SegmentWorkbench.tsx' and target == './thumbs':
                return 'from "./old-thumbs"'
            absolute = (repo / 'src/features/workspace' / target).resolve().as_posix()
            return f'from "{absolute}"'

        source = re.sub(r'from "([^"]+)"', rewrite, source)
        (output / f'old-{filename}').write_text(source, encoding='utf-8')
    print(output)


if __name__ == '__main__':
    main()
