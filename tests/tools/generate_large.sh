#!/bin/sh
# 用途：生成大规模测试数据
# 用法：./generate_large.sh [节点数] [实体输出文件] [关联输出文件]
# 默认：10000 行，输出到 stdout

COUNT="${1:-10000}"
OUTPUT="${2:-/dev/stdout}"
REL_OUTPUT="${3:-/dev/null}"

# 生成实体流
exec > "$OUTPUT"

printf "ROOT\tRoot Node\t/\tlive\n"

for i in $(seq 1 "$COUNT"); do
    PARENT="ROOT"
    if [ $((i % 5)) -eq 0 ]; then
        PARENT="NODE-$((i - 3))"
    elif [ $((i % 7)) -eq 0 ]; then
        PARENT="NODE-$((i - 5))"
    fi
    printf "NODE-%08d\t测试节点 %d\t/path/to/node-%d.md\t%s\n" \
        "$i" "$i" "$i" \
        "$([ $((i % 3)) -eq 0 ] && echo 'archived' || echo 'live')"
done

# 生成关联表（写入 REL_OUTPUT）
if [ "$REL_OUTPUT" != "/dev/null" ]; then
    exec > "$REL_OUTPUT"
    for i in $(seq 1 "$COUNT"); do
        PARENT="ROOT"
        if [ $((i % 5)) -eq 0 ]; then
            PARENT="NODE-$((i - 3))"
        elif [ $((i % 7)) -eq 0 ]; then
            PARENT="NODE-$((i - 5))"
        fi
        printf "%s\tNODE-%08d\n" "$PARENT" "$i"
    done
fi
