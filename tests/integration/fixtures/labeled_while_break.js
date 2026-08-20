// @expected-stdout: 5
// break label on a while loop
let i = 0;
loop1: while (i < 10) {
    i = i + 1;
    if (i === 5) break loop1;
}
console.log(i);
