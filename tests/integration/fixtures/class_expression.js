// @expected-stdout: 42
// Class expression: creates a constructor, new creates instance with methods
const MyClass = class {
  constructor(val) {
    this.val = val;
  }
  getVal() {
    return this.val;
  }
};
let obj = new MyClass(42);
console.log(obj.getVal());
