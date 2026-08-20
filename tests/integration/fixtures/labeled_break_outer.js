// @expected-stdout: 1
// break outer should exit the outer for loop immediately
let result = 0;
outer: for (let i = 0; i < 3; i++) {
    for (let j = 0; j < 3; j++) {
        if (j === 1) break outer;
        result = result + 1;
    }
}
console.log(result);
