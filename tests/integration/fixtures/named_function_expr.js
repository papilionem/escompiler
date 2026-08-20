// Named function expression can reference itself recursively
// @expected-stdout-begin
// 120
// undefined
// @expected-stdout-end
var fact = function factorial(n) {
    if (n <= 1) return 1;
    return n * factorial(n - 1);
};
console.log(fact(5));

// Name not visible outside
console.log(typeof factorial);
