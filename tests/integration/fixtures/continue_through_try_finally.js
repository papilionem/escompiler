// @expected-stdout-begin
// finally 0
// body 1
// finally 1
// finally 2
// done
// @expected-stdout-end
// Continue inside try-finally should execute finally before continuing.
for (let i = 0; i < 3; i++) {
    try {
        if (i === 0 || i === 2) continue;
        console.log("body", i);
    } finally {
        console.log("finally", i);
    }
}
console.log("done");
