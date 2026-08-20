// @expected-stdout-begin
// 0
// finally
// done
// @expected-stdout-end
// Break inside the finally body itself (overrides any pending completion).
for (let i = 0; i < 3; i++) {
    try {
        console.log(i);
    } finally {
        console.log("finally");
        break;
    }
}
console.log("done");
