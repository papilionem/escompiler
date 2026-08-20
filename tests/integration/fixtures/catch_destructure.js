// @expected-stdout-begin
// 1 2
// hello
// @expected-stdout-end
try {
    throw [1, 2];
} catch ([a, b]) {
    console.log(a, b);
}
try {
    throw { message: "hello" };
} catch ({ message }) {
    console.log(message);
}
