# 1 から 10 までの和（while・test・$((…))）
S=0
I=1
while [ $I -le 10 ]; do
    S=$((S + I))
    I=$((I + 1))
done
echo sum=$S
