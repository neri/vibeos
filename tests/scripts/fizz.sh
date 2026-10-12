# FizzBuzz を 1 行に並べる（引数で終わりの数。for・if/elif・変数）
N=${1}
if [ -z "$N" ]; then N=15; fi
R=""
I=1
while [ $I -le $N ]; do
    if [ $((I % 15)) -eq 0 ]; then
        W=FizzBuzz
    elif [ $((I % 3)) -eq 0 ]; then
        W=Fizz
    elif [ $((I % 5)) -eq 0 ]; then
        W=Buzz
    else
        W=$I
    fi
    R="$R $W"
    I=$((I + 1))
done
echo "fizzbuzz:$R"
