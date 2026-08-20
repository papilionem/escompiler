// @expected-stdout-begin
// next: 1
// next: 2
// return called
// done
// @expected-stdout-end
var log = [];
var iterable = {
  [Symbol.iterator]: function() {
    var i = 0;
    return {
      next: function() {
        i++;
        log.push('next: ' + i);
        return { value: i, done: i > 3 };
      },
      return: function() {
        log.push('return called');
        return { value: undefined, done: true };
      }
    };
  }
};

for (var x of iterable) {
  if (x === 2) break;
}
log.push('done');
for (var j = 0; j < log.length; j++) {
  console.log(log[j]);
}
