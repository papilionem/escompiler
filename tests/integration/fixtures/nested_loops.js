// @expected-stdout: 9
let count = 0;
for (let i = 0; i < 3; i = i + 1) {
    for (let j = 0; j < 3; j = j + 1) {
        count = count + 1;
    }
}
console.log(count);
