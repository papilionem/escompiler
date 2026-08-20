// @expected-stdout-begin
// 42
// 100
// @expected-stdout-end
const obj = {
    _val: 42,
    get val() { return this._val; },
    set val(v) { this._val = v; }
};
console.log(obj.val);
obj.val = 100;
console.log(obj.val);
