// @expected-stdout: 5
let i = 0;
while (true) {
    if (i === 5) {
        break;
    }
    i = i + 1;
}
console.log(i);
