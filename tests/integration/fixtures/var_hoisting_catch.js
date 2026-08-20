// @expected-stdout: 1

// var inside catch block should be hoisted to function scope
try {
    throw 1;
} catch (e) {
    var x = e;
}
console.log(x);
