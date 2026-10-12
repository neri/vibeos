# 起動したときに最初の端末が実行する（この sh の中で動くので、変数は後で打つコマンドからも見える）
echo "autorun: hello from $0"
echo "autorun: sum $((1 + 2 + 3))"
AUTO=set-by-autorun
