"""Bounded mixed Chinese/ASCII output for a disposable performance terminal (20 lines/s)."""
import sys,time
for i in range(1500):
    print(f'\x1b[36m{i:06d}\x1b[0m task output / 编译检查 渲染滚动 性能测试 status=running')
sys.stdout.flush()
for i in range(12000):
    print(f'\x1b[32m{i:06d}\x1b[0m task output / 编译检查 渲染滚动 性能测试 status=running',flush=True)
    time.sleep(0.05)
