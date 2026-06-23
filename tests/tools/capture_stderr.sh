#!/bin/sh
# 用途：运行命令并捕获 stderr 输出到临时文件
# 用法：./capture_stderr.sh <命令...>

TMP_LOG=$(mktemp /tmp/stree-test-XXXX.log)
echo "日志文件: $TMP_LOG"

# 执行命令，stderr 重定向到日志文件
"$@" 2> "$TMP_LOG"
EXIT_CODE=$?

echo "--- stderr 输出 ---"
cat "$TMP_LOG"
echo "--- 结束 ---"
echo "退出码: $EXIT_CODE"

# 保留日志文件，供后续检查
echo "日志保留在: $TMP_LOG"
exit $EXIT_CODE
