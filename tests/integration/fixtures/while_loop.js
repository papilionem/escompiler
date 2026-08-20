// @expected-stdout: 10
let sum = 0;
let i = 1;
while (i <= 4) {
    sum = sum + i;
    i = i + 1;
}
console.log(sum);
