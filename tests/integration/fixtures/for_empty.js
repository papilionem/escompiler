// @expected-stdout: done
let i = 0;
for (;;) {
    if (i === 3) {
        break;
    }
    i = i + 1;
}
console.log("done");
