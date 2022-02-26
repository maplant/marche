$(document).ready(function() {
    $(".tag").hover(function() {
        $(this).css({ 'background-color' : 'white'});
        $(this).parents(".thread-row").css({ 'background-color' : ''});
    }, function() {
        $(this).css({ 'background-color' : ''});
        $(this).parents(".thread-row").css({ 'background-color' : 'white'});
    });
    $(".thread-row").hover(function() {
        $(this).parents(".thread-row").css({ 'background-color' : ''});
        $(this).css({ 'background-color' : 'white'});
    }, function() {
        $(this).css({ 'background-color' : ''});
    });
});