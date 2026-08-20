// @expected-stdout: 42
let obj = { x: 42 };
let ref = new WeakRef(obj);
console.log(ref.deref().x);
