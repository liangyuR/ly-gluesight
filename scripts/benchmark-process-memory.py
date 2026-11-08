import argparse
import ctypes as c
from ctypes import wintypes as w
import json
import os
from pathlib import Path
import time

class ProcessEntry(c.Structure):
    _fields_ = [('size', w.DWORD), ('usage', w.DWORD), ('pid', w.DWORD), ('heap', c.c_size_t), ('module', w.DWORD), ('threads', w.DWORD), ('parent', w.DWORD), ('priority', w.LONG), ('flags', w.DWORD), ('exe', w.WCHAR * 260)]

class MemoryCounters(c.Structure):
    _fields_ = [('size', w.DWORD), ('pageFaults', w.DWORD)] + [(name, c.c_size_t) for name in ['peakWorkingSet', 'workingSet', 'quotaPeakPaged', 'quotaPaged', 'quotaPeakNonPaged', 'quotaNonPaged', 'pagefile', 'peakPagefile', 'privateBytes']]

kernel = c.WinDLL('kernel32', use_last_error=True)
psapi = c.WinDLL('psapi', use_last_error=True)
kernel.CreateToolhelp32Snapshot.argtypes = [w.DWORD, w.DWORD]
kernel.CreateToolhelp32Snapshot.restype = w.HANDLE
kernel.Process32FirstW.argtypes = [w.HANDLE, c.POINTER(ProcessEntry)]
kernel.Process32NextW.argtypes = [w.HANDLE, c.POINTER(ProcessEntry)]
kernel.OpenProcess.argtypes = [w.DWORD, w.BOOL, w.DWORD]
kernel.OpenProcess.restype = w.HANDLE
kernel.CloseHandle.argtypes = [w.HANDLE]
psapi.GetProcessMemoryInfo.argtypes = [w.HANDLE, c.POINTER(MemoryCounters), w.DWORD]

def processes():
    handle = kernel.CreateToolhelp32Snapshot(2, 0)
    if handle == c.c_void_p(-1).value:
        raise c.WinError(c.get_last_error())
    result = {}
    try:
        entry = ProcessEntry()
        entry.size = c.sizeof(entry)
        valid = kernel.Process32FirstW(handle, c.byref(entry))
        while valid:
            result[entry.pid] = entry.parent
            valid = kernel.Process32NextW(handle, c.byref(entry))
    finally:
        kernel.CloseHandle(handle)
    return result

def memory(pid):
    handle = kernel.OpenProcess(0x0400 | 0x0010, False, pid)
    if not handle:
        return None
    try:
        values = MemoryCounters()
        values.size = c.sizeof(values)
        if not psapi.GetProcessMemoryInfo(handle, c.byref(values), values.size):
            return None
        return {key: getattr(values, key) for key in ['workingSet', 'privateBytes', 'peakWorkingSet']}
    finally:
        kernel.CloseHandle(handle)

def snapshot(roots):
    parents = processes()
    groups = {}
    for root in roots:
        children = {root}
        while True:
            found = {pid for pid, parent in parents.items() if parent in children}
            if found <= children:
                break
            children.update(found)
        samples = [value for pid in children if (value := memory(pid)) is not None]
        groups[str(root)] = {'processCount': len(samples), **{key: sum(value[key] for value in samples) for key in ['workingSet', 'privateBytes', 'peakWorkingSet']}}
    return {'timeUnixMs': round(time.time() * 1000), 'roots': groups}

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--pids', nargs='+', type=int, default=[os.getpid()])
    parser.add_argument('--output', type=Path)
    parser.add_argument('--stop-file', type=Path)
    parser.add_argument('--interval-ms', type=int, default=100)
    args = parser.parse_args()
    if not args.output:
        print(json.dumps(snapshot(args.pids)))
        return
    if not args.stop_file:
        parser.error('--output requires --stop-file')
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open('w', encoding='utf-8') as stream:
        while not args.stop_file.exists():
            stream.write(json.dumps(snapshot(args.pids)) + '\n')
            stream.flush()
            time.sleep(max(10, args.interval_ms) / 1000)

if __name__ == '__main__':
    main()
