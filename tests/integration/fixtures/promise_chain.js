// @expected-stdout: 4
Promise.resolve(1)
    .then(function(x) { return x + 1; })
    .then(function(x) { return x * 2; })
    .then(function(x) { console.log(x); });
