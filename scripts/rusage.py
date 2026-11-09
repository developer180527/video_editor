#!/usr/bin/env python3
"""What the kernel bills a process for over an interval (Apple platforms).

    scripts/rusage.py <pid> [seconds]

Reads proc_pid_rusage (RUSAGE_INFO_V6), the counters Activity Monitor and
the battery accounting use, twice and prints the rates: CPU (cores), package
idle wakeups/s (the CPU left idle for this process), interrupt wakeups/s,
instructions and cycles per second, billed energy (mW) and the footprint.
"""
import ctypes, sys, time

class V6(ctypes.Structure):
    _fields_ = [(n, ctypes.c_uint64) for n in (
        "uuid0 uuid1 user_time system_time pkg_idle_wkups interrupt_wkups pageins "
        "wired_size resident_size phys_footprint proc_start_abstime proc_exit_abstime "
        "child_user_time child_system_time child_pkg_idle_wkups child_interrupt_wkups "
        "child_pageins child_elapsed_abstime diskio_bytesread diskio_byteswritten "
        "qos_default qos_maintenance qos_background qos_utility qos_legacy "
        "qos_user_initiated qos_user_interactive billed_system_time serviced_system_time "
        "logical_writes lifetime_max_phys_footprint instructions cycles billed_energy "
        "serviced_energy interval_max_phys_footprint runnable_time flags user_ptime system_ptime "
        "pinstructions pcycles energy_nj penergy_nj secure_time secure_ptime neural_footprint "
        "max_neural interval_neural conclave_footprint page_wait_time page_cache_hits "
        "r0 r1 r2 r3 r4 r5").split()]

libproc = ctypes.CDLL("/usr/lib/libproc.dylib")
libc = ctypes.CDLL("/usr/lib/libSystem.dylib")

class Timebase(ctypes.Structure):
    _fields_ = [("numer", ctypes.c_uint32), ("denom", ctypes.c_uint32)]
tb = Timebase()
libc.mach_timebase_info(ctypes.byref(tb))
ticks_ns = tb.numer / tb.denom

def read(pid):
    v = V6()
    if libproc.proc_pid_rusage(pid, 6, ctypes.byref(v)) != 0:
        raise SystemExit(f"proc_pid_rusage({pid}) failed")
    return v, time.monotonic()

def main():
    pid = int(sys.argv[1]); secs = float(sys.argv[2]) if len(sys.argv) > 2 else 10
    a, t0 = read(pid)
    time.sleep(secs)
    b, t1 = read(pid)
    dt = t1 - t0
    d = lambda f: (getattr(b, f) - getattr(a, f)) / dt
    cpu = (d("user_time") + d("system_time")) * ticks_ns / 1e9
    print(f"cpu {cpu * 100:6.1f} %  idle-wakeups {d('pkg_idle_wkups'):7.1f}/s  "
          f"intr-wakeups {d('interrupt_wkups'):7.1f}/s  instr {d('instructions') / 1e6:8.1f} M/s  "
          f"cycles {d('cycles') / 1e6:8.1f} M/s  energy {d('energy_nj') / 1e6:7.1f} mW (P cores {d('penergy_nj') / 1e6:.1f})  "
          f"P-core time {100 * (d('user_ptime') + d('system_ptime')) / max(1, d('user_time') + d('system_time')):.0f} %  "
          f"footprint {b.phys_footprint / 2**20:6.0f} MB (peak {b.lifetime_max_phys_footprint / 2**20:.0f})  "
          f"disk r/w {d('diskio_bytesread') / 1e3:.0f}/{d('diskio_byteswritten') / 1e3:.0f} kB/s")

main()
