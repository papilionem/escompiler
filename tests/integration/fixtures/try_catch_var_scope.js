// @expected-stdout: declaration in catch
// var in catch block should be visible outside (var hoists)
try {
    throw new Error();
} catch (e) {
    var foo = "declaration in catch";
}
console.log(foo);
