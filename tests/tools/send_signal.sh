#!/bin/sh
# 用途：向 stree 进程发送 SIGUSR1 信号触发重载
# 用法：./send_signal.sh <PID>

if [ -z "$1" ]; then
    echo "用法: $0 <PID>"
    exit 1
fi

if ! kill -0 "$1" 2>/dev/null; then
    echo "错误: PID $1 不存在"
    exit 1
fi

kill -SIGUSR1 "$1"
echo "已向 PID $1 发送 SIGUSR1"
