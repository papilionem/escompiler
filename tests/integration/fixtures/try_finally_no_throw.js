// @expected-stdout-begin
// try body
// finally body
// after
// @expected-stdout-end
// try-finally without throw (normal flow)
try {
    console.log("try body");
} finally {
    console.log("finally body");
}
console.log("after");
