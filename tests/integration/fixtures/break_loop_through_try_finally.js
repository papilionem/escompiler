// @expected-stdout-begin
// iteration 0
// finally
// done
// @expected-stdout-end
// Break from a loop where the break is inside try-finally.
for (let i = 0; i < 5; i++) {
    try {
        console.log("iteration", i);
        break;
    } finally {
        console.log("finally");
    }
}
console.log("done");
