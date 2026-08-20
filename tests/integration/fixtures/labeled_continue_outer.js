// @expected-stdout: 3
// continue outer should skip the rest of the inner loop and go to
// the next iteration of the outer loop. Only j===0 runs each time.
let result = 0;
outer: for (let i = 0; i < 3; i++) {
    for (let j = 0; j < 3; j++) {
        if (j === 1) continue outer;
        result = result + 1;
    }
}
console.log(result);
