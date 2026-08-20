// @expected-stdout: 12
let sum = 0;
for (let i = 0; i < 10; i = i + 1) {
    if (i === 5) {
        break;
    }
    if (i % 2 === 0) {
        continue;
    }
    sum = sum + i;
}
let result = sum + 8;
console.log(result);
