// @expected-stdout: 6
let sum = 0;
for (const x of [1, 2, 3]) {
    sum = sum + x;
}
console.log(sum);
