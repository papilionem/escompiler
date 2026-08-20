// @expected-stdout-begin
// 0
// Counter
// 3
// undefined
// @expected-stdout-end

class Counter {
    static count = 0;
    static name = "Counter";
    static computed = 1 + 2;
    static uninitialized;
}

console.log(Counter.count);
console.log(Counter.name);
console.log(Counter.computed);
console.log(Counter.uninitialized);
