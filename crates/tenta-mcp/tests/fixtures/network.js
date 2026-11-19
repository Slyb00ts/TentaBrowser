var post = new XMLHttpRequest();
post.open('POST', '/api/status?code=201');
post.send('payload');
Promise.all([
  fetch('/about.html').then(function (response) { return response.status; }),
  fetch('/api/status?code=404').then(function (response) { return response.status; }),
  fetch('/api/redirect?to=/about.html').then(function (response) { return response.status; }),
  fetch('http://127.0.0.1:1/unreachable').then(
    function () { return 'reached'; },
    function () { return 'failed'; }
  ),
]).then(function (results) {
  document.getElementById('state').textContent = results.join(' ');
});
