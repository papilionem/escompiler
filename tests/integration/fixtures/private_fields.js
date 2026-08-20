// Private fields and methods test
// @expected-stdout-begin
// 42
// 100
// true
// false
// hello from #method
// 10
// @expected-stdout-end

class Counter {
    #count = 0;

    increment() {
        this.#count = this.#count + 1;
    }

    getCount() {
        return this.#count;
    }
}

// Private field get returns correct value
class Box {
    #value;
    constructor(v) {
        this.#value = v;
    }
    getValue() {
        return this.#value;
    }
}

var b = new Box(42);
console.log(b.getValue());

// Private field set updates value
class Container {
    #data = 0;
    set(v) { this.#data = v; }
    get() { return this.#data; }
}
var c = new Container();
c.set(100);
console.log(c.get());

// #x in obj returns true for correct class
class HasPrivate {
    #secret = 1;
    static check(obj) {
        return #secret in obj;
    }
}
var hp = new HasPrivate();
console.log(HasPrivate.check(hp));

// #x in obj returns false for wrong class
console.log(HasPrivate.check({}));

// Private method callable
class WithMethod {
    #greet() {
        return "hello from #method";
    }
    callGreet() {
        return this.#greet();
    }
}
var wm = new WithMethod();
console.log(wm.callGreet());

// Private field with initializer expression
class WithInit {
    #x = 5 + 5;
    getX() { return this.#x; }
}
var wi = new WithInit();
console.log(wi.getX());
