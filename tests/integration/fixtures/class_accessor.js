// @expected-stdout-begin
// 10
// 20
// @expected-stdout-end
class Counter {
    constructor() {
        this._count = 10;
    }
    get count() { return this._count; }
    set count(v) { this._count = v; }
}
let c = new Counter();
console.log(c.count);
c.count = 20;
console.log(c.count);
