#!/bin/bash
# 从 M3U 播源拉取频道并验证流地址可用性
export PATH="/usr/bin:/bin:/usr/sbin:/sbin:$PATH"

PLAYLIST_URL="${1:-https://live.zbds.top/tv/iptv4.m3u}"
SOURCE_NAME="zbds-iptv4"
TEMP_DIR="/tmp/iptv_verify_$$"
mkdir -p "$TEMP_DIR"
M3U_FILE="$TEMP_DIR/playlist.m3u"
CHANNELS_FILE="$TEMP_DIR/channels.txt"

cleanup() {
    rm -rf "$TEMP_DIR"
}
trap cleanup EXIT

echo "========================================"
echo "拉取播源: $PLAYLIST_URL"
echo "========================================"

# 1. 下载 M3U 文件
HTTP_CODE=$(curl -sS -w '%{http_code}' -o "$M3U_FILE" \
    -L --max-time 30 \
    -H 'User-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36' \
    "$PLAYLIST_URL")

if [ "$HTTP_CODE" != "200" ]; then
    echo "❌ 下载失败: HTTP $HTTP_CODE"
    exit 1
fi

FILE_SIZE=$(wc -c < "$M3U_FILE" | tr -d ' ')
if [ "$FILE_SIZE" -eq 0 ]; then
    echo "❌ 播源返回空内容"
    exit 1
fi

if ! head -1 "$M3U_FILE" | grep -q "#EXTM3U"; then
    echo "❌ 不是有效的 M3U 格式"
    head -3 "$M3U_FILE"
    exit 1
fi

echo "✅ 下载成功, 文件大小: ${FILE_SIZE} bytes"

# 2. 检测是否为 Master Playlist (包含 #EXT-X-STREAM-INF)
if grep -q "#EXT-X-STREAM-INF" "$M3U_FILE"; then
    echo ""
    echo "⚠️  检测到主播放列表 (Master Playlist)"
    echo "正在递归拉取第一个子流..."
    
    # 提取第一个子流 URL
    SUB_URL=$(grep -A1 "#EXT-X-STREAM-INF" "$M3U_FILE" | tail -1 | tr -d '\r' | xargs)
    if [ -n "$SUB_URL" ]; then
        echo "子流 URL: $SUB_URL"
        
        # 处理相对 URL
        BASE_URL=$(dirname "$PLAYLIST_URL")
        if [[ ! "$SUB_URL" =~ ^https?:// ]]; then
            if [[ "$SUB_URL" = /* ]]; then
                PROTO=$(echo "$PLAYLIST_URL" | cut -d'/' -f1)
                HOST=$(echo "$PLAYLIST_URL" | cut -d'/' -f3)
                SUB_URL="${PROTO}//${HOST}${SUB_URL}"
            else
                SUB_URL="${BASE_URL}/${SUB_URL}"
            fi
            echo "解析后子流 URL: $SUB_URL"
        fi
        
        curl -sS -L --max-time 30 \
            -H 'User-Agent: Mozilla/5.0' \
            -o "$M3U_FILE" "$SUB_URL"
        
        if [ $? -ne 0 ]; then
            echo "❌ 子流拉取失败"
            exit 1
        fi
        FILE_SIZE=$(wc -c < "$M3U_FILE" | tr -d ' ')
        echo "✅ 子流下载成功, 文件大小: ${FILE_SIZE} bytes"
    fi
fi

# 3. 解析频道列表
echo ""
echo "========================================"
echo "解析频道列表..."
echo "========================================"

> "$CHANNELS_FILE"
CHANNEL_NAME=""
CHANNEL_CATEGORY=""

while IFS= read -r line; do
    line=$(echo "$line" | tr -d '\r' | xargs)
    
    if [[ "$line" == \#EXTINF:* ]]; then
        # 提取显示名称 (逗号后面的部分)
        DISPLAY_NAME=$(echo "$line" | sed 's/.*,//' | xargs)
        CHANNEL_NAME="$DISPLAY_NAME"
        
        # 提取 group-title
        GROUP_TITLE=$(echo "$line" | sed -n 's/.*group-title="\([^"]*\)".*/\1/p')
        if [ -z "$GROUP_TITLE" ]; then
            GROUP_TITLE="-"
        fi
        CHANNEL_CATEGORY="$GROUP_TITLE"
        
    elif [[ "$line" =~ ^https?:// ]] && [ -n "$CHANNEL_NAME" ]; then
        echo "${CHANNEL_CATEGORY}|${CHANNEL_NAME}|${line}" >> "$CHANNELS_FILE"
        CHANNEL_NAME=""
    fi
done < "$M3U_FILE"

TOTAL=$(wc -l < "$CHANNELS_FILE" | tr -d ' ')
echo "📊 总频道数: $TOTAL"

if [ "$TOTAL" -eq 0 ]; then
    echo "❌ 未解析到任何频道"
    echo ""
    echo "文件前 20 行内容:"
    head -20 "$M3U_FILE"
    exit 1
fi

# 按分类统计
echo ""
echo "📊 分类统计:"
cut -d'|' -f1 "$CHANNELS_FILE" | sort | uniq -c | sort -rn | while read count cat; do
    printf "  %-20s : %d 个频道\n" "$cat" "$count"
done

# 展示前 20 个频道
echo ""
echo "📋 频道列表 (前 20 个):"
head -20 "$CHANNELS_FILE" | awk -F'|' '{
    printf "  %3d. %-20s | 分类: %-15s | URL: %s\n", NR, $2, $1, $3
}'

# 4. 验证流地址
echo ""
echo "========================================"
echo "🔍 验证流地址可用性..."
echo "========================================"

VALID=0
INVALID=0
CURRENT=0
BAD_LIST=""

while IFS='|' read -r cat name url; do
    CURRENT=$((CURRENT + 1))
    
    # 使用 curl 检测: 连接成功 + HTTP 200 + 包含 M3U 标记
    RESULT=$(curl -sS -L --max-time 10 --connect-timeout 5 \
        -o /dev/null -w '%{http_code}' \
        -H 'User-Agent: Mozilla/5.0' \
        "$url" 2>/dev/null)
    
    if [ "$RESULT" = "200" ]; then
        # 下载开头检查是否为 M3U/M3U8
        HEAD_CONTENT=$(curl -sS -L --max-time 8 --connect-timeout 5 \
            -H 'User-Agent: Mozilla/5.0' \
            "$url" 2>/dev/null | head -c 4096)
        
        if echo "$HEAD_CONTENT" | grep -qE '#EXTM3U|#EXTINF|#EXT-X-STREAM-INF'; then
            VALID=$((VALID + 1))
            printf "  ✅ [%d/%d] %-25s -> OK\n" "$CURRENT" "$TOTAL" "$name"
        else
            INVALID=$((INVALID + 1))
            printf "  ❌ [%d/%d] %-25s -> 非M3U格式\n" "$CURRENT" "$TOTAL" "$name"
            BAD_LIST="${BAD_LIST}  - ${name} | ${url} | 非M3U格式\n"
        fi
    else
        INVALID=$((INVALID + 1))
        printf "  ❌ [%d/%d] %-25s -> HTTP %s\n" "$CURRENT" "$TOTAL" "$name" "${RESULT:-超时}"
        BAD_LIST="${BAD_LIST}  - ${name} | ${url} | HTTP ${RESULT:-超时}\n"
    fi
done < "$CHANNELS_FILE"

# 5. 结果汇总
echo ""
echo "========================================"
echo "📊 验证结果汇总"
echo "========================================"
echo "  播源: $PLAYLIST_URL"
echo "  总数: $TOTAL"
echo "  有效: $VALID ($(echo "scale=1; $VALID * 100 / $TOTAL" | bc 2>/dev/null || awk "BEGIN {printf \"%.1f\", $VALID * 100 / $TOTAL}")%)"
echo "  无效: $INVALID ($(echo "scale=1; $INVALID * 100 / $TOTAL" | bc 2>/dev/null || awk "BEGIN {printf \"%.1f\", $INVALID * 100 / $TOTAL}")%)"

if [ -n "$BAD_LIST" ]; then
    echo ""
    echo "❌ 无效流地址详情:"
    echo -e "$BAD_LIST"
fi

exit 0
