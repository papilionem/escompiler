// @expected-stdout-begin
// a
// 2
// @expected-stdout-end

// Object.defineProperty with enumerable: false should hide from Object.keys
let obj = {};
obj.a = 1;
Object.defineProperty(obj, 'b', { value: 2, writable: true, enumerable: false, configurable: true });

// Object.keys should only return 'a'
let keys = Object.keys(obj);
console.log(keys.join(','));

// But the value should still be accessible
console.log(obj.b);
